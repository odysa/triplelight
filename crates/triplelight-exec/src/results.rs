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

fn term_ref_json(t: TermRef<'_>, out: &mut String) {
    match t {
        TermRef::NamedNode(n) => { out.push_str("{\"type\":\"uri\",\"value\":"); esc(n.as_str(), out); }
        TermRef::BlankNode(b) => { out.push_str("{\"type\":\"bnode\",\"value\":"); esc(b.as_str(), out); }
        TermRef::Literal(l) => {
            out.push_str("{\"type\":\"literal\",\"value\":"); esc(l.value(), out);
            if let Some(lang) = l.language() { out.push_str(",\"xml:lang\":"); esc(lang, out); }
            else if l.datatype() != triplelight_rdf::vocab::xsd::STRING { out.push_str(",\"datatype\":"); esc(l.datatype().as_str(), out); }
        }
        #[allow(unreachable_patterns)]
        _ => out.push_str("{\"type\":\"literal\",\"value\":\"\""),
    }
    out.push('}');
}

/// Shared JSON framing for decoded rows and borrowed dictionary fragments.
/// Each chunk keeps its leading comma; the combined body drops only the first.
struct SolutionsWriter {
    names: Vec<String>,
}

impl SolutionsWriter {
    fn start(vars: &[Variable], out: &mut String) -> Self {
        out.push_str("{\"head\":{\"vars\":[");
        for (i, v) in vars.iter().enumerate() { if i > 0 { out.push(',') } esc(v.as_str(), out); }
        out.push_str("]},\"results\":{\"bindings\":[");
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

    fn chunks(&self, chunks: Vec<String>, out: &mut String) {
        let body = chunks.concat();
        out.push_str(body.strip_prefix(',').unwrap_or(&body));
    }
}

/// Returns (content type, body).
pub fn serialize(r: &QueryResult) -> (&'static str, String) {
    let mut out = String::new();
    match r {
        QueryResult::Boolean(b) => { let _ = write!(out, "{{\"head\":{{}},\"boolean\":{b}}}"); ("application/sparql-results+json", out) }
        QueryResult::Solutions { vars, rows } => {
            let writer = SolutionsWriter::start(vars, &mut out);
            use rayon::prelude::*;
            let chunks: Vec<String> = rows.par_chunks(4096).map(|chunk| {
                let mut out = String::with_capacity(chunk.len() * 96);
                for row in chunk {
                    writer.row(row.iter().map(Option::as_ref), &mut out, term_json);
                }
                out
            }).collect();
            writer.chunks(chunks, &mut out);
            out.push_str("]}}");
            ("application/sparql-results+json", out)
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
    use rayon::prelude::*;
    let mut out = String::new();
    let writer = SolutionsWriter::start(&sol.vars, &mut out);
    let w = sol.width();
    if w > 0 {
        let chunks: Vec<String> = sol.ids.par_chunks(w * 4096).map(|chunk| -> Result<String, crate::storage::StorageError> {
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
        }).collect::<Result<_, _>>()?;
        writer.chunks(chunks, &mut out);
    } else {
        out.push_str(&vec!["{}"; sol.rows].join(","));
    }
    out.push_str("]}}");
    Ok(out)
}
