//! SPARQL 1.1 Query Results JSON and N-Triples output.
use super::{IdSolutions, QueryResult, UNBOUND};
use crate::model::TermId;
use crate::storage::Dictionary;
use triplelight_rdf::TermRef;
use triplelight_rdf::{Term, Variable};
use std::fmt::Write;

fn esc(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => { let _ = write!(out, "\\u{:04x}", c as u32); }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn term_json(t: &Term, out: &mut String) { term_ref_json(t.as_ref(), out) }

#[cfg(feature = "rdf-12")]
fn direction_str(d: triplelight_rdf::BaseDirection) -> &'static str {
    match d { triplelight_rdf::BaseDirection::Ltr => "ltr", triplelight_rdf::BaseDirection::Rtl => "rtl" }
}

/// True if the term is a SPARQL 1.2 feature (a triple term or a base-direction literal). The JSON writer announces
/// `"version":"1.2"` only for results that contain one, so a 1.1 client sees unchanged output.
#[cfg(feature = "rdf-12")]
fn is_rdf12(t: &Term) -> bool {
    match t.as_ref() { TermRef::Triple(_) => true, TermRef::Literal(l) => l.direction().is_some(), _ => false }
}
#[cfg(not(feature = "rdf-12"))]
fn is_rdf12(_: &Term) -> bool { false }

/// Media type of a SPARQL JSON result, with the 1.2 version parameter when the body announces version 1.2.
const JSON_TYPE: &str = "application/sparql-results+json";
const JSON_TYPE_12: &str = "application/sparql-results+json; version=1.2";

fn term_ref_json(t: TermRef<'_>, out: &mut String) {
    match t {
        TermRef::NamedNode(n) => { out.push_str("{\"type\":\"uri\",\"value\":"); esc(n.as_str(), out); }
        TermRef::BlankNode(b) => { out.push_str("{\"type\":\"bnode\",\"value\":"); esc(b.as_str(), out); }
        TermRef::Literal(l) => {
            out.push_str("{\"type\":\"literal\",\"value\":"); esc(l.value(), out);
            if let Some(lang) = l.language() {
                out.push_str(",\"xml:lang\":"); esc(lang, out);
                // SPARQL 1.2 Query Results JSON: a base direction is the "its:dir" member next to "xml:lang".
                #[cfg(feature = "rdf-12")]
                if let Some(dir) = l.direction() { out.push_str(",\"its:dir\":"); esc(direction_str(dir), out); }
            }
            else if l.datatype() != triplelight_rdf::vocab::xsd::STRING { out.push_str(",\"datatype\":"); esc(l.datatype().as_str(), out); }
        }
        // SPARQL 1.2 Query Results JSON: {"type":"triple","value":{"subject":S,"predicate":P,"object":O}}, recursive.
        #[cfg(feature = "rdf-12")]
        TermRef::Triple(t) => {
            out.push_str("{\"type\":\"triple\",\"value\":{\"subject\":");
            match t.subject.as_ref() {
                triplelight_rdf::NamedOrBlankNodeRef::NamedNode(n) => term_ref_json(TermRef::NamedNode(n), out),
                triplelight_rdf::NamedOrBlankNodeRef::BlankNode(b) => term_ref_json(TermRef::BlankNode(b), out),
            }
            out.push_str(",\"predicate\":"); term_ref_json(TermRef::NamedNode(t.predicate.as_ref()), out);
            out.push_str(",\"object\":"); term_ref_json(t.object.as_ref(), out);
            out.push('}');
        }
    }
    out.push('}');
}

/// Shared JSON framing for decoded rows and borrowed dictionary fragments.
/// Each chunk keeps its leading comma; the combined body drops only the first.
struct SolutionsWriter {
    names: Vec<String>,
}

impl SolutionsWriter {
    fn start(vars: &[Variable], version12: bool, out: &mut String) -> Self {
        out.push_str("{\"head\":{\"vars\":[");
        for (i, v) in vars.iter().enumerate() { if i > 0 { out.push(',') } esc(v.as_str(), out); }
        out.push_str(if version12 { "],\"version\":\"1.2\"},\"results\":{\"bindings\":[" } else { "]},\"results\":{\"bindings\":[" });
        let names = vars.iter().map(|v| { let mut s = String::new(); esc(v.as_str(), &mut s); s.push(':'); s }).collect();
        Self { names }
    }

    fn row<T>(&self, row: impl IntoIterator<Item = Option<T>>, out: &mut String, mut term: impl FnMut(T, &mut String)) {
        out.push_str(",{");
        let mut first = true;
        for (name, value) in self.names.iter().zip(row) {
            let Some(value) = value else { continue };
            if !first { out.push(',') }
            first = false;
            out.push_str(name);
            term(value, out);
        }
        out.push('}');
    }

}

/// Serializes `items` in parallel chunks of `chunk` items and appends them to `out` in order, a bounded window of
/// chunks at a time (task #84). Each chunk's rows start with a comma; the first one's is dropped. Only one window of
/// chunk strings is alive at once, so peak memory is the output plus one window, not the output two or three times
/// over (the earlier collect-then-concat kept every chunk, their concatenation and `out` alive together).
fn append_chunks<T: Sync, E: Send>(items: &[T], chunk: usize, out: &mut String,
                                   f: impl Fn(&[T]) -> Result<String, E> + Sync + Send) -> Result<(), E> {
    use rayon::prelude::*;
    let window = chunk * rayon::current_num_threads().max(1) * 4;
    let mut first = true;
    for part in items.chunks(window) {
        let strings: Vec<String> = part.par_chunks(chunk).map(&f).collect::<Result<_, _>>()?;
        for s in strings {
            out.push_str(if first { s.strip_prefix(',').unwrap_or(&s) } else { &s });
            first = false;
        }
    }
    Ok(())
}

/// Returns (content type, body).
pub fn serialize(r: &QueryResult) -> (&'static str, String) {
    let mut out = String::new();
    match r {
        QueryResult::Boolean(b) => { let _ = write!(out, "{{\"head\":{{}},\"boolean\":{b}}}"); ("application/sparql-results+json", out) }
        QueryResult::Solutions { vars, rows } => {
            let version12 = rows.iter().any(|row| row.iter().flatten().any(is_rdf12));
            let writer = SolutionsWriter::start(vars, version12, &mut out);
            let _ = append_chunks(rows, 4096, &mut out, |chunk| -> Result<String, std::convert::Infallible> {
                let mut out = String::with_capacity(chunk.len() * 96);
                for row in chunk {
                    writer.row(row.iter().map(Option::as_ref), &mut out, term_json);
                }
                Ok(out)
            });
            out.push_str("]}}");
            (if version12 { JSON_TYPE_12 } else { JSON_TYPE }, out)
        }
        QueryResult::Ids(_) => panic!("IdSolutions are serialized with serialize_ids"),
        QueryResult::Graph(ts) => {
            for t in ts { let _ = writeln!(out, "{t} ."); }
            ("application/n-triples", out)
        }
    }
}

/// SPARQL JSON straight from term IDs: each term is written from the dictionary's borrowed bytes, so a large result
/// never materializes owned `Term`s. Rows are serialized in parallel chunks.
pub fn serialize_ids<D: Dictionary + Sync + ?Sized>(sol: &IdSolutions, dict: &D) -> Result<String, crate::storage::StorageError> {
    serialize_ids_v(sol, dict, false)
}

/// Like `serialize_ids`; `store_has_rdf12` is the snapshot's `has_rdf12_terms()`. When it is true, or a computed term is a
/// 1.2 term, the head announces `"version":"1.2"`. A store without 1.2 terms keeps byte-identical 1.1 output.
pub fn serialize_ids_v<D: Dictionary + Sync + ?Sized>(sol: &IdSolutions, dict: &D, store_has_rdf12: bool) -> Result<String, crate::storage::StorageError> {
    let mut out = String::new();
    // 1.2 version announcement from the store-level flag (no peeking at stored terms) or from computed terms (scratch).
    let version12 = store_has_rdf12 || sol.scratch.values().any(is_rdf12);
    let writer = SolutionsWriter::start(&sol.vars, version12, &mut out);
    let w = sol.width();
    if w > 0 {
        append_chunks(&sol.ids, w * 4096, &mut out, |chunk| -> Result<String, crate::storage::StorageError> {
            // Decode every distinct stored ID of this chunk with one batched dictionary call (one cache access per
            // dictionary block on disk), then assemble rows in their original order. Callbacks may arrive in any
            // order; an error discards the whole chunk.
            let mut distinct: Vec<TermId> = chunk.iter().copied().filter(|id| *id != UNBOUND && !sol.scratch.contains_key(id)).collect();
            distinct.sort_unstable();
            distinct.dedup();
            // Fragments go into one arena string (no allocation per term); `span[i]` locates term i.
            let mut arena = String::with_capacity(distinct.len() * 64);
            let mut span: Vec<(u32, u32)> = vec![(0, 0); distinct.len()];
            dict.with_terms(&distinct, &mut |i, t| {
                let start = arena.len();
                term_ref_json(t, &mut arena);
                span[i] = (start as u32, arena.len() as u32);
            })?;
            let mut out = String::with_capacity(chunk.len() / w * 96);
            for row in chunk.chunks_exact(w) {
                writer.row(row.iter().map(|&id| (id != UNBOUND).then_some(id)), &mut out, |id, out| {
                    match sol.scratch.get(&id) {
                        Some(t) => term_json(t, out),
                        None => { let (a, b) = span[distinct.binary_search(&id).expect("decoded above")]; out.push_str(&arena[a as usize..b as usize]) }
                    }
                });
            }
            Ok(out)
        })?;
    } else {
        out.push_str(&vec!["{}"; sol.rows].join(","));
    }
    out.push_str("]}}");
    Ok(out)
}

// ---- SPARQL 1.2 Query Results CSV and TSV (task #104; draft of 2026-11-16, fixtures in tests/fixtures/rdf12-results) ----

/// Turtle-style term for TSV: `<iri>`, `_:label`, `"lex"`, `"lex"@lang`, `"lex"@lang--dir`, `"lex"^^<dt>`, `<<( s p o )>>`.
fn tsv_term(t: TermRef<'_>, out: &mut String) {
    match t {
        TermRef::NamedNode(n) => { out.push('<'); out.push_str(n.as_str()); out.push('>'); }
        TermRef::BlankNode(b) => { out.push_str("_:"); out.push_str(b.as_str()); }
        TermRef::Literal(l) => {
            out.push('"');
            for c in l.value().chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c => out.push(c),
                }
            }
            out.push('"');
            if let Some(lang) = l.language() {
                out.push('@'); out.push_str(lang);
                #[cfg(feature = "rdf-12")]
                if let Some(dir) = l.direction() { out.push_str("--"); out.push_str(direction_str(dir)); }
            } else if l.datatype() != triplelight_rdf::vocab::xsd::STRING {
                out.push_str("^^<"); out.push_str(l.datatype().as_str()); out.push('>');
            }
        }
        #[cfg(feature = "rdf-12")]
        TermRef::Triple(t) => {
            out.push_str("<<( ");
            match t.subject.as_ref() {
                triplelight_rdf::NamedOrBlankNodeRef::NamedNode(n) => tsv_term(TermRef::NamedNode(n), out),
                triplelight_rdf::NamedOrBlankNodeRef::BlankNode(b) => tsv_term(TermRef::BlankNode(b), out),
            }
            out.push(' '); tsv_term(TermRef::NamedNode(t.predicate.as_ref()), out);
            out.push(' '); tsv_term(t.object.as_ref(), out);
            out.push_str(" )>>");
        }
    }
}

/// Lexical form for CSV: the IRI, `_:label`, or the literal's lexical value, with no type markers. A triple term is
/// `<<( s p o )>>` with each part in this form, except that a literal part is wrapped in double quotes with embedded
/// quotes doubled (as in the draft's example).
fn csv_term(t: TermRef<'_>, inner: bool, out: &mut String) {
    match t {
        TermRef::NamedNode(n) => out.push_str(n.as_str()),
        TermRef::BlankNode(b) => { out.push_str("_:"); out.push_str(b.as_str()); }
        TermRef::Literal(l) if inner => { out.push('"'); out.push_str(&l.value().replace('"', "\"\"")); out.push('"'); }
        TermRef::Literal(l) => out.push_str(l.value()),
        #[cfg(feature = "rdf-12")]
        TermRef::Triple(t) => {
            out.push_str("<<( ");
            match t.subject.as_ref() {
                triplelight_rdf::NamedOrBlankNodeRef::NamedNode(n) => csv_term(TermRef::NamedNode(n), true, out),
                triplelight_rdf::NamedOrBlankNodeRef::BlankNode(b) => csv_term(TermRef::BlankNode(b), true, out),
            }
            out.push(' '); csv_term(TermRef::NamedNode(t.predicate.as_ref()), true, out);
            out.push(' '); csv_term(t.object.as_ref(), true, out);
            out.push_str(" )>>");
        }
    }
}

/// RFC 4180 field: quoted, with quotes doubled, when it holds a comma, a quote or a line break.
fn csv_field(s: &str, out: &mut String) {
    if s.contains([',', '"', '\n', '\r']) { out.push('"'); out.push_str(&s.replace('"', "\"\"")); out.push('"'); } else { out.push_str(s); }
}

/// SPARQL Results TSV for a solutions result: a header of `?var` names, then one tab-separated row per solution; an unbound
/// variable is an empty field. Returns `None` for other result kinds (boolean and graph results have no TSV form).
pub fn serialize_tsv(r: &QueryResult) -> Option<(&'static str, String)> {
    let QueryResult::Solutions { vars, rows } = r else { return None };
    let mut out = String::new();
    out.push_str(&vars.iter().map(|v| format!("?{}", v.as_str())).collect::<Vec<_>>().join("\t"));
    out.push('\n');
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i > 0 { out.push('\t') }
            if let Some(t) = cell { tsv_term(t.as_ref(), &mut out) }
        }
        out.push('\n');
    }
    Some(("text/tab-separated-values", out))
}

/// SPARQL Results CSV for a solutions result: a header of variable names, then one row per solution, fields in lexical form
/// (lossy by design); an unbound variable is an empty field. Returns `None` for other result kinds.
pub fn serialize_csv(r: &QueryResult) -> Option<(&'static str, String)> {
    let QueryResult::Solutions { vars, rows } = r else { return None };
    let mut out = String::new();
    for (i, v) in vars.iter().enumerate() { if i > 0 { out.push(',') } csv_field(v.as_str(), &mut out) }
    out.push_str("\r\n");
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i > 0 { out.push(',') }
            if let Some(t) = cell { let mut f = String::new(); csv_term(t.as_ref(), false, &mut f); csv_field(&f, &mut out) }
        }
        out.push_str("\r\n");
    }
    Some(("text/csv", out))
}
