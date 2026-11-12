//! SPARQL 1.1 Query Results JSON and N-Triples output.
use super::{IdSolutions, QueryResult, UNBOUND};
use crate::storage::Dictionary;
use oxrdf::TermRef;
use oxrdf::Term;
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
            else if l.datatype() != oxrdf::vocab::xsd::STRING { out.push_str(",\"datatype\":"); esc(l.datatype().as_str(), out); }
        }
        #[allow(unreachable_patterns)]
        _ => out.push_str("{\"type\":\"literal\",\"value\":\"\""),
    }
    out.push('}');
}

/// Returns (content type, body).
pub fn serialize(r: &QueryResult) -> (&'static str, String) {
    let mut out = String::new();
    match r {
        QueryResult::Boolean(b) => { let _ = write!(out, "{{\"head\":{{}},\"boolean\":{b}}}"); ("application/sparql-results+json", out) }
        QueryResult::Solutions { vars, rows } => {
            out.push_str("{\"head\":{\"vars\":[");
            for (i, v) in vars.iter().enumerate() { if i > 0 { out.push(',') } esc(v.as_str(), &mut out); }
            out.push_str("]},\"results\":{\"bindings\":[");
            use rayon::prelude::*;
            let names: Vec<String> = vars.iter().map(|v| { let mut s = String::new(); esc(v.as_str(), &mut s); s.push(':'); s }).collect();
            let chunks: Vec<String> = rows.par_chunks(4096).map(|chunk| {
                let mut out = String::with_capacity(chunk.len() * 96);
                for row in chunk {
                    out.push_str(",{");
                    let mut first = true;
                    for (name, t) in names.iter().zip(row) {
                        let Some(t) = t else { continue };
                        if !first { out.push(',') }
                        first = false;
                        out.push_str(name); term_json(t, &mut out);
                    }
                    out.push('}');
                }
                out
            }).collect();
            let body: String = chunks.concat();
            out.push_str(body.strip_prefix(',').unwrap_or(&body));
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
pub fn serialize_ids<D: Dictionary + Sync + ?Sized>(sol: &IdSolutions, dict: &D) -> String {
    use rayon::prelude::*;
    let mut out = String::new();
    out.push_str("{\"head\":{\"vars\":[");
    for (i, v) in sol.vars.iter().enumerate() { if i > 0 { out.push(',') } esc(v.as_str(), &mut out); }
    out.push_str("]},\"results\":{\"bindings\":[");
    let w = sol.width();
    if w > 0 {
        let names: Vec<String> = sol.vars.iter().map(|v| { let mut s = String::new(); esc(v.as_str(), &mut s); s.push(':'); s }).collect();
        let chunks: Vec<String> = sol.ids.par_chunks(w * 4096).map(|chunk| {
            let mut out = String::with_capacity(chunk.len() / w * 96);
            for row in chunk.chunks_exact(w) {
                out.push_str(",{");
                let mut first = true;
                for (name, &id) in names.iter().zip(row) {
                    if id == UNBOUND { continue }
                    if !first { out.push(',') }
                    first = false;
                    out.push_str(name);
                    match sol.scratch.get(&id) {
                        Some(t) => term_json(t, &mut out),
                        None => term_ref_json(dict.resolve_ref(id), &mut out),
                    }
                }
                out.push('}');
            }
            out
        }).collect();
        let body = chunks.concat();
        out.push_str(body.strip_prefix(',').unwrap_or(&body));
    }
    out.push_str("]}}");
    out
}
