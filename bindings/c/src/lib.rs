//! C ABI for the embedded TripleLight database, shared by the Go (cgo) and Java (Foreign Function & Memory API)
//! bindings. See `include/triplelight.h` for the contract.
//!
//! Results cross the boundary as one owned buffer: a row-major array of `u32` cells that index a de-duplicated term
//! table, and one UTF-8 byte arena holding every string. Each distinct term is encoded once, so a host language can
//! decode it once and share the object across rows. No Rust panic unwinds into the host: every entry point catches
//! panics and reports them as `TL_ERR_INTERNAL`.
#![allow(clippy::missing_safety_doc)]
use std::collections::HashMap;
use std::ffi::c_char;
use std::panic::{catch_unwind, AssertUnwindSafe};

#[doc(hidden)]
pub mod arrow;
#[doc(hidden)]
pub mod export;
pub use arrow::BATCH_ROWS;
use arrow_array::ffi_stream::FFI_ArrowArrayStream;

use triplelight::embedded::{Database, DatabaseError, ReadSnapshot};
use triplelight::exec::{IdSolutions, QueryResult, UNBOUND};
use triplelight::model::TermId;
use triplelight::rdf::{Term, TermRef};

pub const TL_OK: i32 = 0;
pub const TL_ERR_SYNTAX: i32 = 1;
pub const TL_ERR_UNSUPPORTED: i32 = 2;
pub const TL_ERR_STORAGE: i32 = 3;
pub const TL_ERR_LOCKED: i32 = 4;
pub const TL_ERR_INVALID_ARGUMENT: i32 = 5;
pub const TL_ERR_INTERNAL: i32 = 6;
pub const TL_ERR_GRAPH_STATE: i32 = 7;
/// The query was cancelled or ran past its deadline (not yet reachable from C: no interrupt is exposed).
pub const TL_ERR_INTERRUPTED: i32 = 8;
/// SHACL rejects the shapes graph given to `tl_validate` (a malformed shape or list, or an undefined recursion).
pub const TL_ERR_SHAPES: i32 = 9;
/// The query needed more working memory than its budget (not yet reachable from C: no budget is exposed).
pub const TL_ERR_RESOURCE_LIMIT: i32 = 10;

pub const TL_RESULT_BINDINGS: u32 = 0;
pub const TL_RESULT_BOOLEAN: u32 = 1;
pub const TL_RESULT_TRIPLES: u32 = 2;

pub const TL_TERM_IRI: u8 = 1;
pub const TL_TERM_BLANK: u8 = 2;
pub const TL_TERM_LITERAL: u8 = 3;
pub const TL_TERM_TRIPLE: u8 = 4;
pub const TL_DIR_NONE: u8 = 0;
pub const TL_DIR_LTR: u8 = 1;
pub const TL_DIR_RTL: u8 = 2;

/// An open database. Safe to use from several threads at once; `tl_close` must not race with other calls on it.
pub struct TlDb(Database);

/// An error: a kind (`TL_ERR_*`) and a UTF-8 message.
pub struct TlError { kind: i32, message: String }

/// A slice of the result arena: `off` and `len` are byte offsets into `bytes`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TlStr { pub off: u64, pub len: u64 }

/// One distinct RDF term. For literals `datatype` is always set (an IRI) and `language` is empty unless the literal
/// is language-tagged; for IRIs and blank nodes both are empty.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TlTerm { pub kind: u8, pub direction: u8, pub _pad: [u8; 6], pub value: TlStr, pub language: TlStr, pub datatype: TlStr }

/// Read-only view of a result. Pointers stay valid until `tl_result_free`.
#[repr(C)]
pub struct TlResultView {
    pub kind: u32,
    /// For `TL_RESULT_BOOLEAN`: 0 or 1.
    pub boolean: u32,
    /// Variables in projection order (also for empty results). Zero for booleans; three (subject, predicate, object)
    /// for triples, which have no names.
    pub n_vars: u64,
    pub vars: *const TlStr,
    pub n_rows: u64,
    /// `n_rows * width` cells, row-major, where width is `n_vars` (bindings) or 3 (triples). A cell is a 1-based
    /// index into `terms`; 0 means unbound.
    pub cells: *const u32,
    pub n_terms: u64,
    pub terms: *const TlTerm,
    pub n_bytes: u64,
    pub bytes: *const u8,
}

/// An owned query result.
pub struct TlResult { view: TlResultView, _vars: Vec<TlStr>, _cells: Vec<u32>, _terms: Vec<TlTerm>, _bytes: Vec<u8> }

#[derive(Default)]
struct Builder { bytes: Vec<u8>, terms: Vec<TlTerm>, index: HashMap<Term, u32>, strings: HashMap<String, TlStr> }

impl Builder {
    fn push(&mut self, s: &str) -> TlStr {
        let off = self.bytes.len() as u64;
        self.bytes.extend_from_slice(s.as_bytes());
        TlStr { off, len: s.len() as u64 }
    }
    /// Datatype and language strings repeat across many literals; store each once.
    fn shared(&mut self, s: &str) -> TlStr {
        if let Some(r) = self.strings.get(s) { return *r }
        let r = self.push(s);
        self.strings.insert(s.to_string(), r);
        r
    }
    fn term(&mut self, t: &Term) -> Result<u32, TlError> {
        if let Some(&i) = self.index.get(t) { return Ok(i) }
        let entry = match t {
            Term::NamedNode(n) => TlTerm { kind: TL_TERM_IRI, value: self.push(n.as_str()), ..Default::default() },
            Term::BlankNode(b) => TlTerm { kind: TL_TERM_BLANK, value: self.push(b.as_str()), ..Default::default() },
            Term::Literal(l) => {
                let value = self.push(l.value());
                let language = match l.language() { Some(lang) => self.shared(lang), None => TlStr::default() };
                let datatype = self.shared(l.datatype().as_str());
                #[cfg(feature = "rdf-12")]
                let direction = match l.direction() { Some(triplelight::rdf::BaseDirection::Ltr) => TL_DIR_LTR, Some(triplelight::rdf::BaseDirection::Rtl) => TL_DIR_RTL, None => TL_DIR_NONE };
                #[cfg(not(feature = "rdf-12"))]
                let direction = TL_DIR_NONE;
                TlTerm { kind: TL_TERM_LITERAL, direction, value, language, datatype, ..Default::default() }
            }
            // RDF 1.2 triple term: the three parts are terms of their own (recursively), referenced by 1-based index.
            #[cfg(feature = "rdf-12")]
            Term::Triple(tr) => {
                let s: Term = tr.subject.clone().into();
                let p: Term = tr.predicate.clone().into();
                let (si, pi, oi) = (self.term(&s)?, self.term(&p)?, self.term(&tr.object)?);
                TlTerm { kind: TL_TERM_TRIPLE, value: TlStr { off: si as u64, len: pi as u64 }, language: TlStr { off: oi as u64, len: 0 }, ..Default::default() }
            }
            #[allow(unreachable_patterns)]
            _ => return Err(TlError { kind: TL_ERR_UNSUPPORTED, message: "unsupported term kind".into() }),
        };
        self.terms.push(entry);
        let i = u32::try_from(self.terms.len())
            .map_err(|_| TlError { kind: TL_ERR_INTERNAL, message: "more than 4,294,967,295 distinct terms".into() })?;
        self.index.insert(t.clone(), i);
        Ok(i)
    }
}

fn build(r: QueryResult) -> Result<Box<TlResult>, TlError> {
    let mut b = Builder::default();
    let (kind, boolean, vars, cells, n_rows) = match r {
        QueryResult::Solutions { vars, rows } => {
            let names: Vec<TlStr> = vars.iter().map(|v| b.push(v.as_str())).collect();
            let total = rows.len().checked_mul(vars.len())
                .ok_or_else(|| TlError { kind: TL_ERR_INTERNAL, message: "result too large".into() })?;
            let mut cells = Vec::with_capacity(total);
            for row in &rows {
                for cell in row {
                    cells.push(match cell { Some(t) => b.term(t)?, None => 0 });
                }
            }
            (TL_RESULT_BINDINGS, 0, names, cells, rows.len())
        }
        QueryResult::Boolean(x) => (TL_RESULT_BOOLEAN, x as u32, Vec::new(), Vec::new(), 0),
        QueryResult::Graph(triples) => {
            let mut cells = Vec::with_capacity(triples.len() * 3);
            for t in &triples {
                cells.push(b.term(&t.subject.clone().into())?);
                cells.push(b.term(&Term::NamedNode(t.predicate.clone()))?);
                cells.push(b.term(&t.object)?);
            }
            (TL_RESULT_TRIPLES, 0, vec![TlStr::default(); 3], cells, triples.len())
        }
        QueryResult::Ids(_) => return Err(TlError { kind: TL_ERR_INTERNAL, message: "undecoded result".into() }),
    };
    Ok(finish(b, kind, boolean, vars, cells, n_rows))
}

fn finish(b: Builder, kind: u32, boolean: u32, vars: Vec<TlStr>, cells: Vec<u32>, n_rows: usize) -> Box<TlResult> {
    let mut out = Box::new(TlResult {
        view: TlResultView {
            kind, boolean, n_vars: vars.len() as u64, vars: std::ptr::null(), n_rows: n_rows as u64, cells: std::ptr::null(),
            n_terms: b.terms.len() as u64, terms: std::ptr::null(), n_bytes: b.bytes.len() as u64, bytes: std::ptr::null(),
        },
        _vars: vars, _cells: cells, _terms: b.terms, _bytes: b.bytes,
    });
    out.view.vars = out._vars.as_ptr();
    out.view.cells = out._cells.as_ptr();
    out.view.terms = out._terms.as_ptr();
    out.view.bytes = out._bytes.as_ptr();
    out
}

/// SELECT results as term IDs: every distinct stored ID is resolved once, through one batched dictionary pass on the
/// same snapshot that produced the IDs, and written straight into the arena (no owned `Term`, no hashing of decoded
/// terms). Computed values (scratch IDs) go through the owned-term path.
fn build_ids(snap: &ReadSnapshot, sol: IdSolutions) -> Result<Box<TlResult>, TlError> {
    let mut b = Builder::default();
    let names: Vec<TlStr> = sol.vars.iter().map(|v| b.push(v.as_str())).collect();
    let mut distinct: Vec<TermId> = sol.ids.iter().copied().filter(|id| *id != UNBOUND && !sol.scratch.contains_key(id)).collect();
    distinct.sort_unstable();
    distinct.dedup();
    u32::try_from(distinct.len()).map_err(|_| TlError { kind: TL_ERR_INTERNAL, message: "more than 4,294,967,295 distinct terms".into() })?;
    b.terms = vec![TlTerm::default(); distinct.len()];
    let mut unsupported = false;
    // RDF 1.2 triple terms stored in the dictionary: their parts are added to the term table after the closure (a part may not
    // be among the distinct IDs of this result), then the triple's own slot is filled with the indices of its parts.
    #[cfg(feature = "rdf-12")]
    let mut pending: Vec<(usize, Term)> = Vec::new();
    {
        let Builder { bytes, terms, strings, .. } = &mut b;
        let mut push = |s: &str| { let off = bytes.len() as u64; bytes.extend_from_slice(s.as_bytes()); TlStr { off, len: s.len() as u64 } };
        snap.with_terms(&distinct, &mut |i, t| {
            terms[i] = match t {
                TermRef::NamedNode(n) => TlTerm { kind: TL_TERM_IRI, value: push(n.as_str()), ..Default::default() },
                TermRef::BlankNode(n) => TlTerm { kind: TL_TERM_BLANK, value: push(n.as_str()), ..Default::default() },
                TermRef::Literal(l) => {
                    let value = push(l.value());
                    let mut shared = |s: &str| match strings.get(s) { Some(r) => *r, None => { let r = push(s); strings.insert(s.to_string(), r); r } };
                    let language = l.language().map(&mut shared).unwrap_or_default();
                    let datatype = shared(l.datatype().as_str());
                    #[cfg(feature = "rdf-12")]
                    let direction = match l.direction() { Some(triplelight::rdf::BaseDirection::Ltr) => TL_DIR_LTR, Some(triplelight::rdf::BaseDirection::Rtl) => TL_DIR_RTL, None => TL_DIR_NONE };
                    #[cfg(not(feature = "rdf-12"))]
                    let direction = TL_DIR_NONE;
                    TlTerm { kind: TL_TERM_LITERAL, direction, value, language, datatype, ..Default::default() }
                }
                #[cfg(feature = "rdf-12")]
                TermRef::Triple(t) => { pending.push((i, Term::from(t.clone()))); TlTerm::default() }
                #[allow(unreachable_patterns)]
                _ => { unsupported = true; TlTerm::default() }
            };
        }).map_err(db_error)?;
    }
    #[cfg(feature = "rdf-12")]
    for (i, t) in pending {
        let Term::Triple(tr) = &t else { continue };
        let subject: Term = tr.subject.clone().into();
        let predicate: Term = tr.predicate.clone().into();
        let (si, pi, oi) = (b.term(&subject)?, b.term(&predicate)?, b.term(&tr.object)?);
        b.terms[i] = TlTerm { kind: TL_TERM_TRIPLE, value: TlStr { off: si as u64, len: pi as u64 }, language: TlStr { off: oi as u64, len: 0 }, ..Default::default() };
    }
    if unsupported { return Err(TlError { kind: TL_ERR_UNSUPPORTED, message: "unsupported term kind".into() }) }
    let width = sol.vars.len();
    let total = sol.rows.checked_mul(width).ok_or_else(|| TlError { kind: TL_ERR_INTERNAL, message: "result too large".into() })?;
    let mut cells = Vec::with_capacity(total);
    for &id in &sol.ids {
        cells.push(if id == UNBOUND {
            0
        } else if let Some(t) = sol.scratch.get(&id) {
            b.term(t)?
        } else {
            distinct.binary_search(&id).map(|i| i as u32 + 1).map_err(|_| TlError { kind: TL_ERR_INTERNAL, message: "undecoded term ID".into() })?
        });
    }
    Ok(finish(b, TL_RESULT_BINDINGS, 0, names, cells, sol.rows))
}

fn db_error(e: DatabaseError) -> TlError {
    let kind = match &e {
        DatabaseError::Syntax(_) => TL_ERR_SYNTAX,
        DatabaseError::Unsupported(_) => TL_ERR_UNSUPPORTED,
        DatabaseError::Storage(_) => TL_ERR_STORAGE,
        DatabaseError::Locked => TL_ERR_LOCKED,
        DatabaseError::GraphState(_) => TL_ERR_GRAPH_STATE,
        DatabaseError::Interrupted { .. } => TL_ERR_INTERRUPTED,
        DatabaseError::ResourceLimit(_) => TL_ERR_RESOURCE_LIMIT,
    };
    TlError { kind, message: e.to_string() }
}

fn invalid(message: &str) -> TlError { TlError { kind: TL_ERR_INVALID_ARGUMENT, message: message.into() } }

unsafe fn text<'a>(ptr: *const c_char, len: usize) -> Result<&'a str, TlError> {
    if ptr.is_null() && len != 0 { return Err(invalid("null string pointer")) }
    let bytes = if len == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(ptr as *const u8, len) } };
    std::str::from_utf8(bytes).map_err(|_| invalid("string is not valid UTF-8"))
}

unsafe fn handle<'a>(ptr: *const TlDb) -> Result<&'a Database, TlError> {
    if ptr.is_null() { return Err(invalid("null database handle")) }
    Ok(unsafe { &(*ptr).0 })
}

/// Runs `f`, turning errors and panics into a status code and an owned `TlError` (if `err` is non-null).
unsafe fn guard(err: *mut *mut TlError, f: impl FnOnce() -> Result<(), TlError>) -> i32 {
    let outcome = catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|p| {
        let message = p.downcast_ref::<&str>().map(|s| s.to_string())
            .or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into());
        Err(TlError { kind: TL_ERR_INTERNAL, message })
    });
    match outcome {
        Ok(()) => TL_OK,
        Err(e) => {
            let kind = e.kind;
            if !err.is_null() { unsafe { *err = Box::into_raw(Box::new(e)) } }
            kind
        }
    }
}

macro_rules! out { ($p:expr, $v:expr) => {{ if $p.is_null() { return Err(invalid("null output pointer")) } *$p = $v }} }

/// Opens (or creates) a persistent database in `path`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_open(path: *const c_char, path_len: usize, out: *mut *mut TlDb, err: *mut *mut TlError) -> i32 {
    unsafe { guard(err, || {
        if out.is_null() { return Err(invalid("null output pointer")) }
        let p = text(path, path_len)?;
        let d = Database::open(p).map_err(db_error)?;
        out!(out, Box::into_raw(Box::new(TlDb(d))));
        Ok(())
    }) }
}

/// Creates an isolated, non-durable in-memory database.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_open_in_memory(out: *mut *mut TlDb, err: *mut *mut TlError) -> i32 {
    unsafe { guard(err, || {
        if out.is_null() { return Err(invalid("null output pointer")) }
        *out = Box::into_raw(Box::new(TlDb(Database::in_memory())));
        Ok(())
    }) }
}

/// Closes a database handle. Null is ignored.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_close(db: *mut TlDb) {
    if !db.is_null() { let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(db) }))); }
}

/// Runs a SPARQL query (SELECT, ASK or CONSTRUCT) and returns an owned result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_query(db: *const TlDb, sparql: *const c_char, len: usize, out: *mut *mut TlResult, err: *mut *mut TlError) -> i32 {
    unsafe { tl_query_with_entailment(db, sparql, len, TL_ENTAILMENT_SIMPLE, out, err) }
}

/// `entailment` of `tl_query_with_entailment`: the stored triples (`tl_query`'s regime).
pub const TL_ENTAILMENT_SIMPLE: i32 = 0;
/// `entailment` of `tl_query_with_entailment`: the default graph's RDFS closure, computed at query time (#108).
pub const TL_ENTAILMENT_RDFS: i32 = 1;

/// `tl_query` under an entailment regime, `TL_ENTAILMENT_SIMPLE` or `TL_ENTAILMENT_RDFS`; any other value is
/// `TL_ERR_INVALID_ARGUMENT`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_query_with_entailment(db: *const TlDb, sparql: *const c_char, len: usize, entailment: i32, out: *mut *mut TlResult, err: *mut *mut TlError) -> i32 {
    unsafe { guard(err, || {
        if out.is_null() { return Err(invalid("null output pointer")) }
        let entailment = match entailment {
            TL_ENTAILMENT_SIMPLE => triplelight::exec::Entailment::Simple,
            TL_ENTAILMENT_RDFS => triplelight::exec::Entailment::Rdfs,
            _ => return Err(invalid("unknown entailment")),
        };
        let (d, q) = (handle(db)?, text(sparql, len)?);
        let snap = d.read();
        let r = match snap.query_ids_with_entailment(q, entailment).map_err(db_error)? {
            QueryResult::Ids(ids) => build_ids(&snap, ids)?,
            other => build(other)?,
        };
        out!(out, Box::into_raw(r));
        Ok(())
    }) }
}

/// `tl_validate` document formats.
pub const TL_FORMAT_TURTLE: i32 = 0;
pub const TL_FORMAT_NTRIPLES: i32 = 1;

/// SHACL Core validation (#108) of a data document against a shapes document, each `TL_FORMAT_TURTLE` or
/// `TL_FORMAT_NTRIPLES`. On success `*conforms` is 1 or 0 and `*report` is the report graph (`TL_RESULT_TRIPLES`, free
/// it with `tl_result_free`); data that violates the shapes is a success with `*conforms` 0. A document that does not
/// parse is `TL_ERR_SYNTAX`, shapes SHACL rejects are `TL_ERR_SHAPES`, a feature this build lacks `TL_ERR_UNSUPPORTED`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_validate(data: *const c_char, data_len: usize, data_format: i32, shapes: *const c_char, shapes_len: usize, shapes_format: i32,
                                     conforms: *mut i32, report: *mut *mut TlResult, err: *mut *mut TlError) -> i32 {
    use triplelight::rdf::Format;
    use triplelight::shacl::{DocumentError, ValidationError};
    unsafe { guard(err, || {
        if conforms.is_null() || report.is_null() { return Err(invalid("null output pointer")) }
        let format = |f| match f { TL_FORMAT_TURTLE => Ok(Format::Turtle), TL_FORMAT_NTRIPLES => Ok(Format::NTriples), _ => Err(invalid("unknown format")) };
        let (df, sf) = (format(data_format)?, format(shapes_format)?);
        let (d, s) = (text(data, data_len)?, text(shapes, shapes_len)?);
        let r = triplelight::shacl::validate_documents(d.as_bytes(), df, s.as_bytes(), sf).map_err(|e| {
            let kind = match &e {
                DocumentError::Syntax { .. } => TL_ERR_SYNTAX,
                DocumentError::Validation(ValidationError::Unsupported(_)) => TL_ERR_UNSUPPORTED,
                DocumentError::Validation(_) => TL_ERR_SHAPES,
            };
            TlError { kind, message: e.to_string() }
        })?;
        let graph = build(QueryResult::Graph(r.to_triples()))?;
        out!(conforms, i32::from(r.conforms));
        out!(report, Box::into_raw(graph));
        Ok(())
    }) }
}

/// Runs a SELECT query and returns its solutions as an Arrow C Stream written into caller-allocated `*out`
/// (see `struct ArrowArrayStream` in the header). On failure `*out` is left untouched. ASK and CONSTRUCT are
/// `TL_ERR_UNSUPPORTED`. The stream owns its snapshot and stays valid after `tl_close`; batches it returns are
/// independently owned and stay valid after the stream is released. Pulls must be serialized by the consumer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_query_arrow(db: *const TlDb, sparql: *const c_char, len: usize, out: *mut FFI_ArrowArrayStream, err: *mut *mut TlError) -> i32 {
    unsafe { guard(err, || {
        if out.is_null() { return Err(invalid("null output pointer")) }
        let (d, q) = (handle(db)?, text(sparql, len)?);
        let snap = d.read();
        let ids = match snap.query_ids(q).map_err(db_error)? {
            QueryResult::Ids(ids) => ids,
            _ => return Err(TlError { kind: TL_ERR_UNSUPPORTED, message: "Arrow output supports SELECT only".into() }),
        };
        let reader = arrow::IdBatchReader::new(snap, ids);
        export::export(Box::new(reader), out);
        Ok(())
    }) }
}

/// Applies a SPARQL Update; `changed` receives the number of triples inserted or deleted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_update(db: *const TlDb, sparql: *const c_char, len: usize, changed: *mut u64, err: *mut *mut TlError) -> i32 {
    unsafe { guard(err, || {
        let (d, q) = (handle(db)?, text(sparql, len)?);
        let n = d.update(q).map_err(db_error)?;
        if !changed.is_null() { *changed = n as u64 }
        Ok(())
    }) }
}

/// Loads an N-Triples (`.nt`) or Turtle (`.ttl`) file; `loaded` receives the number of triples added.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_load(db: *const TlDb, path: *const c_char, len: usize, loaded: *mut u64, err: *mut *mut TlError) -> i32 {
    unsafe { guard(err, || {
        let (d, p) = (handle(db)?, text(path, len)?);
        let n = d.load(p).map_err(db_error)?;
        if !loaded.is_null() { *loaded = n as u64 }
        Ok(())
    }) }
}

/// Rewrites the current state into a new immutable base (persistent) or rebuilds the indexes (in-memory).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_compact(db: *const TlDb, err: *mut *mut TlError) -> i32 {
    unsafe { guard(err, || { handle(db)?.compact().map_err(db_error) }) }
}

/// 1 when planner statistics for the current base are ready, else 0 (queries may run either way).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_stats_ready(db: *const TlDb) -> i32 {
    if db.is_null() { return 0 }
    catch_unwind(AssertUnwindSafe(|| unsafe { (*db).0.stats_ready() } as i32)).unwrap_or(0)
}

/// The read-only view of a result; valid until `tl_result_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_result_get_view(r: *const TlResult) -> *const TlResultView {
    if r.is_null() { std::ptr::null() } else { unsafe { &(*r).view } }
}

/// Frees a result. Null is ignored.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_result_free(r: *mut TlResult) {
    if !r.is_null() { drop(unsafe { Box::from_raw(r) }) }
}

/// The error kind (`TL_ERR_*`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_error_kind(e: *const TlError) -> i32 { if e.is_null() { 0 } else { unsafe { (*e).kind } } }

/// The UTF-8 error message (not NUL-terminated); `len` receives its byte length. Valid until `tl_error_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_error_message(e: *const TlError, len: *mut usize) -> *const u8 {
    if e.is_null() { if !len.is_null() { unsafe { *len = 0 } } return std::ptr::null() }
    let m = unsafe { &(*e).message };
    if !len.is_null() { unsafe { *len = m.len() } }
    m.as_ptr()
}

/// Frees an error. Null is ignored.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_error_free(e: *mut TlError) {
    if !e.is_null() { drop(unsafe { Box::from_raw(e) }) }
}

/// Benchmark hooks (not part of the C ABI): encode one query result through the owned-term path or the ID path.
#[doc(hidden)]
pub fn bench_encode(db: &Database, sparql: &str, via_ids: bool) -> (usize, usize) {
    let r = if via_ids {
        let snap = db.read();
        match snap.query_ids(sparql).unwrap() { QueryResult::Ids(ids) => build_ids(&snap, ids), other => build(other) }
    } else {
        build(db.query(sparql).unwrap())
    }.ok().unwrap();
    (r._terms.len(), r._bytes.len())
}

/// The ABI version of this library (`TL_ABI_VERSION` in the header); hosts check it before use.
#[unsafe(no_mangle)]
pub extern "C" fn tl_abi_version() -> u32 { 1 }

// Reports the linked Rayon pool width for the matched Java benchmark.
#[doc(hidden)]
#[cfg(feature = "benchmark-rayon-witness")]
#[unsafe(no_mangle)]
pub extern "C" fn triplelight_bench_rayon_threads() -> u32 {
    std::panic::catch_unwind(rayon::current_num_threads)
        .ok()
        .and_then(|width| u32::try_from(width).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "benchmark-rayon-witness")]
    #[test]
    fn benchmark_rayon_witness_reports_a_positive_width() {
        assert!(triplelight_bench_rayon_threads() >= 1);
    }

    fn s(v: &TlResultView, x: TlStr) -> String {
        let b = unsafe { std::slice::from_raw_parts(v.bytes, v.n_bytes as usize) };
        String::from_utf8(b[x.off as usize..(x.off + x.len) as usize].to_vec()).unwrap()
    }

    unsafe fn q(db: *const TlDb, sparql: &str) -> Result<*mut TlResult, (i32, String)> {
        let (mut r, mut e) = (std::ptr::null_mut(), std::ptr::null_mut());
        let code = unsafe { tl_query(db, sparql.as_ptr() as _, sparql.len(), &mut r, &mut e) };
        if code == TL_OK { return Ok(r) }
        let mut n = 0;
        let m = unsafe { std::slice::from_raw_parts(tl_error_message(e, &mut n), n) };
        let out = (unsafe { tl_error_kind(e) }, String::from_utf8_lossy(m).into_owned());
        unsafe { tl_error_free(e) };
        Err(out)
    }

    #[test]
    fn shacl_validate_reports_and_separates_errors() { unsafe {
        let shapes = "@prefix sh: <http://www.w3.org/ns/shacl#> . <urn:S> a sh:NodeShape ; sh:targetClass <urn:Person> ; sh:property [ sh:path <urn:name> ; sh:minCount 1 ] .";
        let run = |data: &str, shapes: &str| {
            let (mut conforms, mut report, mut e) = (-1, std::ptr::null_mut(), std::ptr::null_mut());
            let code = tl_validate(data.as_ptr() as _, data.len(), TL_FORMAT_TURTLE, shapes.as_ptr() as _, shapes.len(), TL_FORMAT_TURTLE, &mut conforms, &mut report, &mut e);
            if code != TL_OK { tl_error_free(e); return Err(code) }
            let v = &*tl_result_get_view(report);
            let triples = (v.kind, v.n_rows);
            tl_result_free(report);
            Ok((conforms, triples))
        };
        let (conforms, (kind, _)) = run("<urn:ann> a <urn:Person> ; <urn:name> \"Ann\" .", shapes).unwrap();
        assert_eq!((conforms, kind), (1, TL_RESULT_TRIPLES));
        let (conforms, (_, n)) = run("<urn:bob> a <urn:Person> .", shapes).unwrap();
        assert!(conforms == 0 && n > 2, "a violation is a report with results");
        assert_eq!(run("<urn:bob> a", shapes), Err(TL_ERR_SYNTAX));
        assert_eq!(run("", "@prefix sh: <http://www.w3.org/ns/shacl#> . <urn:S> a sh:NodeShape ; sh:targetClass <urn:P> ; sh:property [ sh:minCount 1 ] ."), Err(TL_ERR_SHAPES));
    } }

    #[test]
    fn rdfs_entailment_is_opt_in_and_checked() { unsafe {
        let mut db = std::ptr::null_mut();
        assert_eq!(tl_open_in_memory(&mut db, std::ptr::null_mut()), TL_OK);
        let up = "INSERT DATA { <urn:ann> a <urn:Student> . <urn:Student> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <urn:Person> }";
        assert_eq!(tl_update(db, up.as_ptr() as _, up.len(), std::ptr::null_mut(), std::ptr::null_mut()), TL_OK);
        let sparql = "SELECT ?x WHERE { ?x a <urn:Person> }";
        let rows = |entailment| {
            let mut r = std::ptr::null_mut();
            let code = tl_query_with_entailment(db, sparql.as_ptr() as _, sparql.len(), entailment, &mut r, std::ptr::null_mut());
            if code != TL_OK { return Err(code) }
            let n = (*tl_result_get_view(r)).n_rows;
            tl_result_free(r);
            Ok(n)
        };
        assert_eq!(rows(TL_ENTAILMENT_SIMPLE), Ok(0));
        assert_eq!(rows(TL_ENTAILMENT_RDFS), Ok(1));
        assert_eq!(rows(7), Err(TL_ERR_INVALID_ARGUMENT));
        tl_close(db);
    } }

    #[test]
    fn typed_terms_unbound_dedup_and_empty_projection() { unsafe {
        let mut db = std::ptr::null_mut();
        assert_eq!(tl_open_in_memory(&mut db, std::ptr::null_mut()), TL_OK);
        let up = r#"INSERT DATA { <urn:a> <urn:p> "01"^^<http://www.w3.org/2001/XMLSchema#int> .
            <urn:a> <urn:p> "hi"@en . <urn:a> <urn:q> _:b . <urn:c> <urn:p> "x" }"#;
        let mut changed = 0;
        assert_eq!(tl_update(db, up.as_ptr() as _, up.len(), &mut changed, std::ptr::null_mut()), TL_OK);
        assert_eq!(changed, 4);
        let r = q(db, "SELECT ?s ?o ?z WHERE { ?s <urn:p> ?o OPTIONAL { ?s <urn:none> ?z } } ORDER BY ?s ?o").unwrap();
        let v = &*tl_result_get_view(r);
        assert_eq!(v.kind, TL_RESULT_BINDINGS);
        let vars: Vec<_> = std::slice::from_raw_parts(v.vars, v.n_vars as usize).iter().map(|x| s(v, *x)).collect();
        assert_eq!(vars, ["s", "o", "z"]);
        assert_eq!(v.n_rows, 3);
        let cells = std::slice::from_raw_parts(v.cells, (v.n_rows * v.n_vars) as usize);
        let terms = std::slice::from_raw_parts(v.terms, v.n_terms as usize);
        assert!(cells.iter().skip(2).step_by(3).all(|&c| c == 0), "?z unbound");
        assert_eq!(cells[0], cells[3], "<urn:a> stored once");
        let lit = |c: u32| { let t = terms[c as usize - 1]; (t.kind, s(v, t.value), s(v, t.language), s(v, t.datatype)) };
        let objs: Vec<_> = (0..3).map(|i| lit(cells[i * 3 + 1])).collect();
        assert!(objs.contains(&(TL_TERM_LITERAL, "01".into(), "".into(), "http://www.w3.org/2001/XMLSchema#int".into())));
        assert!(objs.contains(&(TL_TERM_LITERAL, "hi".into(), "en".into(), "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString".into())));
        assert!(objs.contains(&(TL_TERM_LITERAL, "x".into(), "".into(), "http://www.w3.org/2001/XMLSchema#string".into())));
        tl_result_free(r);
        let r = q(db, "SELECT ?x ?y WHERE { ?x <urn:nothing> ?y }").unwrap();
        let v = &*tl_result_get_view(r);
        assert_eq!((v.n_vars, v.n_rows), (2, 0));
        tl_result_free(r);
        let r = q(db, "ASK { <urn:a> <urn:q> ?b }").unwrap();
        assert_eq!(((*tl_result_get_view(r)).kind, (*tl_result_get_view(r)).boolean), (TL_RESULT_BOOLEAN, 1));
        tl_result_free(r);
        let r = q(db, "CONSTRUCT { ?s <urn:r> ?o } WHERE { ?s <urn:q> ?o }").unwrap();
        let v = &*tl_result_get_view(r);
        assert_eq!((v.kind, v.n_rows, v.n_vars), (TL_RESULT_TRIPLES, 1, 3));
        let t = std::slice::from_raw_parts(v.terms, v.n_terms as usize);
        let c = std::slice::from_raw_parts(v.cells, 3);
        assert_eq!(t[c[2] as usize - 1].kind, TL_TERM_BLANK);
        tl_result_free(r);
        tl_close(db);
    } }

    #[test]
    fn id_path_covers_computed_terms_zero_columns_and_construct() { unsafe {
        let mut db = std::ptr::null_mut();
        tl_open_in_memory(&mut db, std::ptr::null_mut());
        let up = r#"INSERT DATA { <urn:a> <urn:p> 1 , 2 . <urn:b> <urn:p> 3 . <urn:a> <urn:q> "x" }"#;
        tl_update(db, up.as_ptr() as _, up.len(), std::ptr::null_mut(), std::ptr::null_mut());
        // Group keys are stored IDs, aggregates and BIND values are computed (scratch) terms.
        let r = q(db, "SELECT ?s (SUM(?o) AS ?n) (CONCAT(STR(?s), \"!\") AS ?c) WHERE { ?s <urn:p> ?o } GROUP BY ?s ORDER BY ?s").unwrap();
        let v = &*tl_result_get_view(r);
        assert_eq!((v.n_rows, v.n_vars), (2, 3));
        let cells = std::slice::from_raw_parts(v.cells, 6);
        let terms = std::slice::from_raw_parts(v.terms, v.n_terms as usize);
        let get = |c: u32| { let t = terms[c as usize - 1]; (t.kind, s(v, t.value)) };
        assert_eq!(get(cells[0]), (TL_TERM_IRI, "urn:a".into()));
        assert_eq!(get(cells[1]), (TL_TERM_LITERAL, "3".into()));
        assert_eq!(get(cells[2]), (TL_TERM_LITERAL, "urn:a!".into()));
        assert_eq!(get(cells[4]), (TL_TERM_LITERAL, "3".into()));
        tl_result_free(r);
        // A computed value equal to a stored term shares its table entry; an OPTIONAL miss is unbound.
        let r = q(db, "SELECT ?s ?k ?z WHERE { ?s <urn:q> ?o BIND(<urn:a> AS ?k) OPTIONAL { ?s <urn:none> ?z } }").unwrap();
        let v = &*tl_result_get_view(r);
        let cells = std::slice::from_raw_parts(v.cells, 3);
        assert_eq!(cells[0], cells[1], "BIND(<urn:a>) must reuse the stored <urn:a> entry");
        assert_eq!(cells[2], 0, "?z is unbound");
        tl_result_free(r);
        // A pattern with no variables: zero columns, one solution.
        let r = q(db, "SELECT * WHERE { <urn:a> <urn:q> \"x\" }").unwrap();
        let v = &*tl_result_get_view(r);
        assert_eq!((v.kind, v.n_vars, v.n_rows), (TL_RESULT_BINDINGS, 0, 1));
        tl_result_free(r);
        let r = q(db, "SELECT * WHERE { <urn:a> <urn:q> \"nope\" }").unwrap();
        assert_eq!((*tl_result_get_view(r)).n_rows, 0);
        tl_result_free(r);
        let r = q(db, "CONSTRUCT { ?s <urn:r> ?o } WHERE { ?s <urn:p> ?o }").unwrap();
        let v = &*tl_result_get_view(r);
        assert_eq!((v.kind, v.n_rows), (TL_RESULT_TRIPLES, 3));
        tl_result_free(r);
        tl_close(db);
    } }

    #[test]
    fn errors_and_invalid_arguments() { unsafe {
        let mut db = std::ptr::null_mut();
        tl_open_in_memory(&mut db, std::ptr::null_mut());
        assert_eq!(q(db, "SELEC nope").unwrap_err().0, TL_ERR_SYNTAX);
        let bad = [0xffu8, 0xfe];
        let mut r = std::ptr::null_mut();
        assert_eq!(tl_query(db, bad.as_ptr() as _, 2, &mut r, std::ptr::null_mut()), TL_ERR_INVALID_ARGUMENT);
        assert_eq!(tl_query(std::ptr::null(), "ASK{}".as_ptr() as _, 5, &mut r, std::ptr::null_mut()), TL_ERR_INVALID_ARGUMENT);
        assert_eq!(tl_query(db, "ASK{}".as_ptr() as _, 5, std::ptr::null_mut(), std::ptr::null_mut()), TL_ERR_INVALID_ARGUMENT);
        let dir = std::env::temp_dir().join(format!("tl-c-nullout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.to_str().unwrap();
        assert_eq!(tl_open(p.as_ptr() as _, p.len(), std::ptr::null_mut(), std::ptr::null_mut()), TL_ERR_INVALID_ARGUMENT);
        assert!(!dir.exists(), "a rejected open must not create the directory");
        let mut e = std::ptr::null_mut();
        let missing = "/nonexistent/missing.nt";
        assert_eq!(tl_load(db, missing.as_ptr() as _, missing.len(), std::ptr::null_mut(), &mut e), TL_ERR_STORAGE);
        tl_error_free(e);
        tl_close(db);
    } }

    #[test]
    fn persistent_and_memory_agree_and_lock() { unsafe {
        let dir = std::env::temp_dir().join(format!("tl-c-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.to_str().unwrap();
        let (mut disk, mut mem) = (std::ptr::null_mut(), std::ptr::null_mut());
        assert_eq!(tl_open(p.as_ptr() as _, p.len(), &mut disk, std::ptr::null_mut()), TL_OK);
        let mut again = std::ptr::null_mut();
        assert_eq!(tl_open(p.as_ptr() as _, p.len(), &mut again, std::ptr::null_mut()), TL_ERR_LOCKED);
        tl_open_in_memory(&mut mem, std::ptr::null_mut());
        let up = "INSERT DATA { <urn:a> <urn:p> 1 , 2 , 3 }";
        for d in [disk, mem] { assert_eq!(tl_update(d, up.as_ptr() as _, up.len(), std::ptr::null_mut(), std::ptr::null_mut()), TL_OK) }
        let sum = |d| { let r = q(d, "SELECT (SUM(?o) AS ?n) WHERE { ?s ?p ?o }").unwrap(); let v = &*tl_result_get_view(r);
            let t = *v.terms; let out = s(v, t.value); tl_result_free(r); out };
        assert_eq!(sum(disk), "6");
        assert_eq!(sum(mem), "6");
        tl_close(disk); tl_close(mem);
        let _ = std::fs::remove_dir_all(&dir);
    } }
}
