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

use triplelight::embedded::{Database, DatabaseError};
use triplelight::exec::QueryResult;
use triplelight::rdf::Term;

pub const TL_OK: i32 = 0;
pub const TL_ERR_SYNTAX: i32 = 1;
pub const TL_ERR_UNSUPPORTED: i32 = 2;
pub const TL_ERR_STORAGE: i32 = 3;
pub const TL_ERR_LOCKED: i32 = 4;
pub const TL_ERR_INVALID_ARGUMENT: i32 = 5;
pub const TL_ERR_INTERNAL: i32 = 6;

pub const TL_RESULT_BINDINGS: u32 = 0;
pub const TL_RESULT_BOOLEAN: u32 = 1;
pub const TL_RESULT_TRIPLES: u32 = 2;

pub const TL_TERM_IRI: u8 = 1;
pub const TL_TERM_BLANK: u8 = 2;
pub const TL_TERM_LITERAL: u8 = 3;

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
pub struct TlTerm { pub kind: u8, pub _pad: [u8; 7], pub value: TlStr, pub language: TlStr, pub datatype: TlStr }

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
                TlTerm { kind: TL_TERM_LITERAL, value, language, datatype, ..Default::default() }
            }
            #[allow(unreachable_patterns)]
            _ => return Err(TlError { kind: TL_ERR_UNSUPPORTED, message: "RDF-star terms are not supported".into() }),
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
    Ok(out)
}

fn db_error(e: DatabaseError) -> TlError {
    let kind = match &e {
        DatabaseError::Syntax(_) => TL_ERR_SYNTAX,
        DatabaseError::Unsupported(_) => TL_ERR_UNSUPPORTED,
        DatabaseError::Storage(_) => TL_ERR_STORAGE,
        DatabaseError::Locked => TL_ERR_LOCKED,
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
        let p = text(path, path_len)?;
        let d = Database::open(p).map_err(db_error)?;
        out!(out, Box::into_raw(Box::new(TlDb(d))));
        Ok(())
    }) }
}

/// Creates an isolated, non-durable in-memory database.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_open_in_memory(out: *mut *mut TlDb, err: *mut *mut TlError) -> i32 {
    unsafe { guard(err, || { out!(out, Box::into_raw(Box::new(TlDb(Database::in_memory())))); Ok(()) }) }
}

/// Closes a database handle. Null is ignored.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_close(db: *mut TlDb) {
    if !db.is_null() { let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(db) }))); }
}

/// Runs a SPARQL query (SELECT, ASK or CONSTRUCT) and returns an owned result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tl_query(db: *const TlDb, sparql: *const c_char, len: usize, out: *mut *mut TlResult, err: *mut *mut TlError) -> i32 {
    unsafe { guard(err, || {
        let (d, q) = (handle(db)?, text(sparql, len)?);
        let r = build(d.query(q).map_err(db_error)?)?;
        out!(out, Box::into_raw(r));
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

/// The ABI version of this library (`TL_ABI_VERSION` in the header); hosts check it before use.
#[unsafe(no_mangle)]
pub extern "C" fn tl_abi_version() -> u32 { 1 }

#[cfg(test)]
mod tests {
    use super::*;

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
    fn errors_and_invalid_arguments() { unsafe {
        let mut db = std::ptr::null_mut();
        tl_open_in_memory(&mut db, std::ptr::null_mut());
        assert_eq!(q(db, "SELEC nope").unwrap_err().0, TL_ERR_SYNTAX);
        let bad = [0xffu8, 0xfe];
        let mut r = std::ptr::null_mut();
        assert_eq!(tl_query(db, bad.as_ptr() as _, 2, &mut r, std::ptr::null_mut()), TL_ERR_INVALID_ARGUMENT);
        assert_eq!(tl_query(std::ptr::null(), "ASK{}".as_ptr() as _, 5, &mut r, std::ptr::null_mut()), TL_ERR_INVALID_ARGUMENT);
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
