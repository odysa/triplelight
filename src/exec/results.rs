//! SPARQL 1.1 Query Results JSON and N-Triples output.
use super::QueryResult;
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

fn term_json(t: &Term, out: &mut String) {
    match t {
        Term::NamedNode(n) => { out.push_str("{\"type\":\"uri\",\"value\":"); esc(n.as_str(), out); }
        Term::BlankNode(b) => { out.push_str("{\"type\":\"bnode\",\"value\":"); esc(b.as_str(), out); }
        Term::Literal(l) => {
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
            for (k, row) in rows.iter().enumerate() {
                if k > 0 { out.push(',') }
                out.push('{');
                let mut first = true;
                for (v, t) in vars.iter().zip(row) {
                    let Some(t) = t else { continue };
                    if !first { out.push(',') }
                    first = false;
                    esc(v.as_str(), &mut out); out.push(':'); term_json(t, &mut out);
                }
                out.push('}');
            }
            out.push_str("]}}");
            ("application/sparql-results+json", out)
        }
        QueryResult::Graph(ts) => {
            for t in ts { let _ = writeln!(out, "{t} ."); }
            ("application/n-triples", out)
        }
    }
}
