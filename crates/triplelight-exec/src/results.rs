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
    emit_chunks(items, chunk, f, |s| { out.push_str(s); Ok::<(), std::convert::Infallible>(()) }).map_err(|e| match e { EmitError::Chunk(e) => e, EmitError::Sink(never) => match never {} })
}

/// Why `emit_chunks` stopped: a chunk failed to serialize, or the sink refused the bytes.
enum EmitError<E, K> { Chunk(E), Sink(K) }

/// Serializes `items` in parallel windows and hands each chunk, in order, to `sink` as soon as its window is ready. The
/// leading comma of the first chunk is dropped. Memory is one window, not the whole body.
fn emit_chunks<T: Sync, E: Send, K>(items: &[T], chunk: usize, f: impl Fn(&[T]) -> Result<String, E> + Sync + Send,
                                    mut sink: impl FnMut(&str) -> Result<(), K>) -> Result<(), EmitError<E, K>> {
    use rayon::prelude::*;
    let window = chunk * rayon::current_num_threads().max(1) * 4;
    let mut first = true;
    for part in items.chunks(window) {
        let strings: Vec<String> = part.par_chunks(chunk).map(&f).collect::<Result<_, _>>().map_err(EmitError::Chunk)?;
        for s in strings {
            sink(if first { s.strip_prefix(',').unwrap_or(&s) } else { &s }).map_err(EmitError::Sink)?;
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
    match write_ids_json(sol, dict, store_has_rdf12, |s| { out.push_str(s); Ok::<(), std::convert::Infallible>(()) }) {
        Ok(()) => Ok(out),
        Err(EmitError::Chunk(e)) => Err(e),
        Err(EmitError::Sink(never)) => match never {},
    }
}

/// Why `serialize_ids_to` stopped. After the first bytes are written the output is incomplete: callers must not treat it as an answer.
#[derive(Debug)]
pub enum WriteIdsError { Storage(crate::storage::StorageError), Io(std::io::Error) }
impl std::fmt::Display for WriteIdsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { Self::Storage(e) => write!(f, "{e}"), Self::Io(e) => write!(f, "{e}") }
    }
}
impl std::error::Error for WriteIdsError {}

/// The bytes of `serialize_ids_v`, written to `w` window by window instead of built as one `String`. Peak memory is one window of
/// rows; the first bytes leave before the later windows are serialized. On error the output so far is a truncated body.
pub fn serialize_ids_to<D: Dictionary + Sync + ?Sized>(sol: &IdSolutions, dict: &D, store_has_rdf12: bool, w: &mut dyn std::io::Write) -> Result<(), WriteIdsError> {
    write_ids_json(sol, dict, store_has_rdf12, |s| w.write_all(s.as_bytes())).map_err(|e| match e {
        EmitError::Chunk(e) => WriteIdsError::Storage(e),
        EmitError::Sink(e) => WriteIdsError::Io(e),
    })
}

fn write_ids_json<D: Dictionary + Sync + ?Sized, K>(sol: &IdSolutions, dict: &D, store_has_rdf12: bool,
                                                    mut sink: impl FnMut(&str) -> Result<(), K>) -> Result<(), EmitError<crate::storage::StorageError, K>> {
    let mut head = String::new();
    // 1.2 version announcement from the store-level flag (no peeking at stored terms) or from computed terms (scratch).
    let version12 = store_has_rdf12 || sol.scratch.values().any(is_rdf12);
    let writer = SolutionsWriter::start(&sol.vars, version12, &mut head);
    sink(&head).map_err(EmitError::Sink)?;
    let w = sol.width();
    if w > 0 {
        emit_chunks(&sol.ids, w * 4096, |chunk| -> Result<String, crate::storage::StorageError> {
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
        }, &mut sink)?;
    } else {
        sink(&vec!["{}"; sol.rows].join(",")).map_err(EmitError::Sink)?;
    }
    sink("]}}").map_err(EmitError::Sink)
}

// ---- SPARQL 1.2 Query Results CSV and TSV (task #104; draft of 2026-11-16, fixtures in tests/fixtures/rdf12-results) ----

/// Numeric and boolean literals whose lexical form is already a Turtle token print without quotes or datatype
/// (`123`, `1.5`, `1.5e3`, `true`), as in the spec's TSV examples.
fn turtle_bare(datatype: &str, v: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let signed = |s: &str| s.strip_prefix(['+', '-']).unwrap_or(s).to_string();
    match datatype {
        "http://www.w3.org/2001/XMLSchema#integer" => digits(&signed(v)),
        "http://www.w3.org/2001/XMLSchema#decimal" => { let u = signed(v); match u.split_once('.') { Some((a, b)) => digits(b) && (a.is_empty() || digits(a)), None => false } }
        "http://www.w3.org/2001/XMLSchema#double" => {
            let u = signed(v);
            match u.split_once(['e', 'E']) {
                Some((m, e)) => digits(&signed(e)) && (digits(m) || m.split_once('.').is_some_and(|(a, b)| (a.is_empty() || digits(a)) && (b.is_empty() || digits(b)) && !(a.is_empty() && b.is_empty()))),
                None => false,
            }
        }
        "http://www.w3.org/2001/XMLSchema#boolean" => v == "true" || v == "false",
        _ => false,
    }
}

/// Turtle-style term for TSV: `<iri>`, `_:label`, `"lex"`, `"lex"@lang`, `"lex"@lang--dir`, `"lex"^^<dt>`, `<<( s p o )>>`.
fn tsv_term(t: TermRef<'_>, out: &mut String) {
    match t {
        TermRef::NamedNode(n) => { out.push('<'); out.push_str(n.as_str()); out.push('>'); }
        TermRef::BlankNode(b) => { out.push_str("_:"); out.push_str(b.as_str()); }
        TermRef::Literal(l) if turtle_bare(l.datatype().as_str(), l.value()) => out.push_str(l.value()),
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
    Some(("text/tab-separated-values; charset=utf-8", out))
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
    Some(("text/csv; charset=utf-8", out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use triplelight_rdf::{BlankNode, Literal, NamedNode};

    fn iri(s: &str) -> Term { NamedNode::new_unchecked(s).into() }
    fn lit(s: &str) -> Term { Literal::new_simple_literal(s).into() }
    fn result(vars: &[&str], rows: Vec<Vec<Option<Term>>>) -> QueryResult {
        QueryResult::Solutions { vars: vars.iter().map(|v| Variable::new_unchecked(*v)).collect(), rows }
    }
    fn lf(s: &str) -> String { s.replace("\r\n", "\n") }

    /// The mixed example of the SPARQL 1.2 CSV/TSV drafts (2026-11-16), without the directional literal.
    fn mixed() -> QueryResult {
        let b = |l: &str| -> Term { BlankNode::new_unchecked(l).into() };
        result(&["x", "literal"], vec![
            vec![Some(iri("http://example/x")), Some(lit("String"))],
            vec![Some(iri("http://example/x")), Some(lit("String-with-dquote\""))],
            vec![Some(b("b0")), Some(lit("Blank node"))],
            vec![None, Some(lit("Missing 'x'"))],
            vec![None, None],
            vec![Some(iri("http://example/x")), None],
            vec![Some(b("b1")), Some(Literal::new_language_tagged_literal_unchecked("String-with-lang", "en").into())],
            vec![Some(b("b1")), Some(Literal::new_typed_literal("123", NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#integer")).into())],
        ])
    }

    #[test]
    fn tsv_follows_the_spec_example() {
        let (ct, body) = serialize_tsv(&mixed()).unwrap();
        assert_eq!(ct, "text/tab-separated-values; charset=utf-8");
        assert_eq!(body, "?x\t?literal\n<http://example/x>\t\"String\"\n<http://example/x>\t\"String-with-dquote\\\"\"\n_:b0\t\"Blank node\"\n\t\"Missing 'x'\"\n\t\n<http://example/x>\t\n_:b1\t\"String-with-lang\"@en\n_:b1\t123\n");
    }

    #[test]
    fn csv_follows_the_spec_example() {
        let (ct, body) = serialize_csv(&mixed()).unwrap();
        assert_eq!(ct, "text/csv; charset=utf-8");
        assert_eq!(lf(&body), "x,literal\nhttp://example/x,String\nhttp://example/x,\"String-with-dquote\"\"\"\n_:b0,Blank node\n,Missing 'x'\n,\nhttp://example/x,\n_:b1,String-with-lang\n_:b1,123\n");
    }

    #[test]
    fn boolean_and_graph_results_have_no_csv_or_tsv_form() {
        assert!(serialize_tsv(&QueryResult::Boolean(true)).is_none());
        assert!(serialize_csv(&QueryResult::Boolean(true)).is_none());
    }

    #[cfg(feature = "rdf-12")]
    mod rdf12 {
        use super::*;
        use triplelight_rdf::{BaseDirection, Triple};

        fn triple(s: &str, p: &str, o: Term) -> Term { Triple::new(NamedNode::new_unchecked(s), NamedNode::new_unchecked(p), o).into() }

        /// The triple-term example of the drafts: a plain string, and a triple term holding IRIs and a literal with quotes.
        fn triples() -> QueryResult {
            result(&["x", "triple"], vec![
                vec![Some(lit("Alice")), Some(triple("http://example/alice", "http://example/knows", iri("http://example/bob")))],
                vec![Some(lit("Carol")), Some(triple("http://example/carol", "http://example/says", lit("Hello world, my name is \"Alice\".")))],
            ])
        }

        #[test]
        fn tsv_triple_terms() {
            let (_, body) = serialize_tsv(&triples()).unwrap();
            assert_eq!(body, "?x\t?triple\n\"Alice\"\t<<( <http://example/alice> <http://example/knows> <http://example/bob> )>>\n\"Carol\"\t<<( <http://example/carol> <http://example/says> \"Hello world, my name is \\\"Alice\\\".\" )>>\n");
        }

        #[test]
        fn csv_triple_terms() {
            let (_, body) = serialize_csv(&triples()).unwrap();
            // Only the triple column is compared with the draft: its x column is printed quoted there, unlike the draft's other CSV example.
            let lines: Vec<String> = lf(&body).lines().map(str::to_string).collect();
            assert_eq!(lines[0], "x,triple");
            assert_eq!(lines[1], "Alice,<<( http://example/alice http://example/knows http://example/bob )>>");
            assert_eq!(lines[2], "Carol,\"<<( http://example/carol http://example/says \"\"Hello world, my name is \"\"\"\"Alice\"\"\"\".\"\" )>>\"");
        }

        #[test]
        fn json_triple_term_and_direction_announce_version_1_2() {
            let dir: Term = Literal::new_directional_language_tagged_literal("String-with-lang-dir", "en", BaseDirection::Ltr).unwrap().into();
            let r = result(&["x", "t"], vec![vec![Some(dir), Some(triple("http://example/a", "http://example/p", lit("v")))]]);
            let (ct, body) = serialize(&r);
            assert_eq!(ct, "application/sparql-results+json; version=1.2");
            assert!(body.starts_with("{\"head\":{\"vars\":[\"x\",\"t\"],\"version\":\"1.2\"}"), "{body}");
            assert!(body.contains("\"xml:lang\":\"en\",\"its:dir\":\"ltr\""), "{body}");
            assert!(body.contains("{\"type\":\"triple\",\"value\":{\"subject\":{\"type\":\"uri\",\"value\":\"http://example/a\"},\"predicate\":{\"type\":\"uri\",\"value\":\"http://example/p\"},\"object\":{\"type\":\"literal\",\"value\":\"v\"}}}"), "{body}");
        }

        #[test]
        fn a_result_without_1_2_terms_keeps_the_1_1_output() {
            let (ct, body) = serialize(&mixed());
            assert_eq!(ct, "application/sparql-results+json");
            assert!(!body.contains("version"), "{body}");
        }

        #[test]
        fn non_ascii_directional_literal_through_both_writers() {
            let dir: Term = Literal::new_directional_language_tagged_literal("שלום", "he", BaseDirection::Rtl).unwrap().into();
            let r = result(&["l"], vec![vec![Some(dir)]]);
            assert_eq!(serialize_tsv(&r).unwrap().1, "?l\n\"שלום\"@he--rtl\n");
            assert_eq!(lf(&serialize_csv(&r).unwrap().1), "l\nשלום\n");
        }

        #[test]
        fn tsv_directional_literal() {
            let dir: Term = Literal::new_directional_language_tagged_literal("String-with-lang-dir", "en", BaseDirection::Ltr).unwrap().into();
            let (_, body) = serialize_tsv(&result(&["l"], vec![vec![Some(dir)]])).unwrap();
            assert_eq!(body, "?l\n\"String-with-lang-dir\"@en--ltr\n");
        }
    }
}
