use triplelight_rdf::{BlankNode, Literal, NamedNode, Term, TermRef, Triple, Variable};
use std::collections::HashMap;
use triplelight::exec::{IdSolutions, QueryResult, UNBOUND, results::{serialize, serialize_ids, serialize_ids_to, WriteIdsError}};
use triplelight::model::TermId;
use triplelight::storage::{Dictionary, StorageError};

struct ReverseDictionary {
    terms: Vec<Term>,
    fail: Option<TermId>,
}

impl Dictionary for ReverseDictionary {
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError> {
        Ok(self.terms.iter().position(|t| t == term).map(|i| TermId(i as u64 + 1)))
    }

    fn resolve(&self, id: TermId) -> Result<Term, StorageError> {
        Ok(self.terms[id.0 as usize - 1].clone())
    }

    fn with_term(&self, id: TermId, f: &mut dyn FnMut(TermRef<'_>)) -> Result<(), StorageError> {
        f(self.terms[id.0 as usize - 1].as_ref());
        Ok(())
    }

    fn with_terms(&self, ids: &[TermId], f: &mut dyn FnMut(usize, TermRef<'_>)) -> Result<(), StorageError> {
        for (i, &id) in ids.iter().enumerate().rev() {
            if self.fail == Some(id) {
                return Err(StorageError::new(std::io::ErrorKind::PermissionDenied, "planted batch failure"));
            }
            self.with_term(id, &mut |term| f(i, term))?;
        }
        Ok(())
    }
}

fn variables(names: &[&str]) -> Vec<Variable> {
    names.iter().map(|name| Variable::new(*name).unwrap()).collect()
}

#[test]
fn decoded_and_id_rows_keep_exact_json_bytes() {
    let terms: Vec<Term> = vec![
        NamedNode::new("urn:iri").unwrap().into(),
        BlankNode::new("b0").unwrap().into(),
        Literal::new_simple_literal("\0\u{1}\u{8}\u{c}\n\r\t\"\\/雪😀").into(),
        Literal::new_language_tagged_literal("hello", "en").unwrap().into(),
        Literal::new_typed_literal("01", triplelight_rdf::vocab::xsd::INT).into(),
    ];
    let vars = variables(&["iri", "blank", "text", "lang", "typed", "absent"]);
    let mut row: Vec<_> = terms.iter().cloned().map(Some).collect();
    row.push(None);
    // Include a fully unbound row and a row whose first bindings are absent.
    let rows = vec![row, vec![None; 6], vec![None, None, None, Some(terms[3].clone()), None, None]];
    let owned = QueryResult::Solutions { vars: vars.clone(), rows };
    let expected = concat!(
        r#"{"head":{"vars":["iri","blank","text","lang","typed","absent"]},"results":{"bindings":["#,
        r#"{"iri":{"type":"uri","value":"urn:iri"},"blank":{"type":"bnode","value":"b0"},"text":{"type":"literal","value":"\u0000\u0001\u0008\u000c\n\r\t\"\\/雪😀"},"lang":{"type":"literal","value":"hello","xml:lang":"en"},"typed":{"type":"literal","value":"01","datatype":"http://www.w3.org/2001/XMLSchema#int"}},"#,
        r#"{},{"lang":{"type":"literal","value":"hello","xml:lang":"en"}}]}}"#,
    );
    assert_eq!(serialize(&owned), ("application/sparql-results+json", expected.into()));

    // The typed value is scratch-owned; stored dictionary callbacks arrive backwards.
    let scratch_id = TermId(0xe000_0000_0000_0001);
    let ids = IdSolutions {
        vars,
        ids: vec![TermId(1), TermId(2), TermId(3), TermId(4), scratch_id, UNBOUND,
                  UNBOUND, UNBOUND, UNBOUND, UNBOUND, UNBOUND, UNBOUND,
                  UNBOUND, UNBOUND, UNBOUND, TermId(4), UNBOUND, UNBOUND],
        scratch: HashMap::from([(scratch_id, terms[4].clone())]),
        rows: 3,
    };
    let dict = ReverseDictionary { terms: terms[..4].to_vec(), fail: None };
    assert_eq!(serialize_ids(&ids, &dict).unwrap(), expected);
}

#[test]
fn empty_projection_and_zero_width_multiplicity_are_preserved() {
    let dict = ReverseDictionary { terms: Vec::new(), fail: None };
    for count in [0, 1, 3] {
        let body = match count {
            0 => "",
            1 => "{}",
            3 => "{},{},{}",
            _ => unreachable!(),
        };
        let expected = format!("{{\"head\":{{\"vars\":[]}},\"results\":{{\"bindings\":[{body}]}}}}");
        let ids = IdSolutions { vars: vec![], ids: vec![], scratch: HashMap::new(), rows: count };
        let owned = QueryResult::Solutions { vars: vec![], rows: vec![vec![]; count] };
        assert_eq!(serialize_ids(&ids, &dict).unwrap(), expected);
        assert_eq!(serialize(&owned).1, expected);
    }
    let expected = r#"{"head":{"vars":["雪"]},"results":{"bindings":[]}}"#;
    let ids = IdSolutions { vars: variables(&["雪"]), ids: vec![], scratch: HashMap::new(), rows: 0 };
    let owned = QueryResult::Solutions { vars: variables(&["雪"]), rows: vec![] };
    assert_eq!(serialize_ids(&ids, &dict).unwrap(), expected);
    assert_eq!(serialize(&owned).1, expected);
}

#[test]
fn parallel_chunks_keep_row_order_duplicates_and_separators() {
    let dict = ReverseDictionary {
        terms: vec![Literal::new_simple_literal("left").into(), Literal::new_simple_literal("right").into()],
        fail: None,
    };
    // Cross two 4096-row boundaries and finish with a single-row chunk.
    let mut raw = Vec::new();
    let mut decoded = Vec::new();
    let mut expected_rows = Vec::new();
    for i in 0..8193 {
        let value = if i % 3 == 0 { None } else { Some(i % 2) };
        raw.push(value.map_or(UNBOUND, |n| TermId(n as u64 + 1)));
        decoded.push(vec![value.map(|n| dict.terms[n].clone())]);
        expected_rows.push(match value {
            None => "{}",
            Some(0) => r#"{"x":{"type":"literal","value":"left"}}"#,
            Some(_) => r#"{"x":{"type":"literal","value":"right"}}"#,
        });
    }
    let expected = format!("{{\"head\":{{\"vars\":[\"x\"]}},\"results\":{{\"bindings\":[{}]}}}}", expected_rows.join(","));
    let ids = IdSolutions { vars: variables(&["x"]), ids: raw, scratch: HashMap::new(), rows: 8193 };
    let owned = QueryResult::Solutions { vars: variables(&["x"]), rows: decoded };
    assert_eq!(serialize_ids(&ids, &dict).unwrap(), expected);
    assert_eq!(serialize(&owned).1, expected);
}

#[test]
fn dictionary_failure_discards_partial_json_and_preserves_error() {
    let dict = ReverseDictionary {
        terms: vec![Literal::new_simple_literal("first").into(), Literal::new_simple_literal("second").into()],
        fail: Some(TermId(1)),
    };
    // Reverse callbacks write the second fragment before the planted failure.
    let ids = IdSolutions {
        vars: variables(&["x"]), ids: vec![TermId(1), TermId(2)], scratch: HashMap::new(), rows: 2,
    };
    let error = serialize_ids(&ids, &dict).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(error.to_string(), "planted batch failure");
}

#[test]
fn boolean_and_graph_content_types_and_bytes_stay_separate() {
    assert_eq!(serialize(&QueryResult::Boolean(true)), ("application/sparql-results+json", r#"{"head":{},"boolean":true}"#.into()));
    assert_eq!(serialize(&QueryResult::Boolean(false)), ("application/sparql-results+json", r#"{"head":{},"boolean":false}"#.into()));
    let graph = QueryResult::Graph(vec![Triple::new(
        NamedNode::new("urn:s").unwrap(), NamedNode::new("urn:p").unwrap(), Literal::new_simple_literal("hello"),
    )]);
    assert_eq!(serialize(&graph), ("application/n-triples", "<urn:s> <urn:p> \"hello\" .\n".into()));
}

/// Counts writes and optionally refuses everything after `limit` bytes.
struct CountingWriter { bytes: Vec<u8>, writes: usize, limit: usize }
impl std::io::Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len() + buf.len() > self.limit {
            return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "client went away"));
        }
        self.writes += 1;
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

fn two_term_dictionary() -> ReverseDictionary {
    ReverseDictionary {
        terms: vec![Literal::new_simple_literal("left").into(), Literal::new_simple_literal("right").into()],
        fail: None,
    }
}

fn many_rows(n: usize) -> IdSolutions {
    let ids = (0..n).map(|i| if i % 3 == 0 { UNBOUND } else { TermId((i % 2) as u64 + 1) }).collect();
    IdSolutions { vars: variables(&["x"]), ids, scratch: HashMap::new(), rows: n }
}

#[test]
fn streamed_json_is_byte_identical_to_the_buffered_body() {
    let dict = two_term_dictionary();
    for n in [0, 1, 4095, 4096, 4097, 8193, 70_000] {
        let ids = many_rows(n);
        let mut sink = CountingWriter { bytes: Vec::new(), writes: 0, limit: usize::MAX };
        serialize_ids_to(&ids, &dict, false, &mut sink).unwrap();
        assert_eq!(String::from_utf8(sink.bytes).unwrap(), serialize_ids(&ids, &dict).unwrap(), "{n} rows");
    }
    let empty = IdSolutions { vars: vec![], ids: vec![], scratch: HashMap::new(), rows: 3 };
    let mut sink = CountingWriter { bytes: Vec::new(), writes: 0, limit: usize::MAX };
    serialize_ids_to(&empty, &dict, false, &mut sink).unwrap();
    assert_eq!(String::from_utf8(sink.bytes).unwrap(), serialize_ids(&empty, &dict).unwrap());
}

#[test]
fn streamed_json_writes_the_head_before_the_rows_and_the_tail_last() {
    let dict = two_term_dictionary();
    let mut sink = CountingWriter { bytes: Vec::new(), writes: 0, limit: usize::MAX };
    serialize_ids_to(&many_rows(70_000), &dict, false, &mut sink).unwrap();
    // The head, at least one window of rows and the closing braces are separate writes.
    assert!(sink.writes >= 3, "{} writes", sink.writes);
    assert!(sink.bytes.ends_with(b"]}}"));
}

#[test]
fn a_refusing_writer_stops_the_stream_with_an_io_error() {
    let dict = two_term_dictionary();
    let mut sink = CountingWriter { bytes: Vec::new(), writes: 0, limit: 200 };
    match serialize_ids_to(&many_rows(70_000), &dict, false, &mut sink) {
        Err(WriteIdsError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe),
        other => panic!("expected an io error, got {other:?}"),
    }
    // What was written is a prefix of the full body that never reaches the closing braces.
    assert!(!sink.bytes.ends_with(b"]}}"));
    assert!(serialize_ids(&many_rows(70_000), &dict).unwrap().as_bytes().starts_with(&sink.bytes));
}

#[test]
fn a_dictionary_failure_surfaces_as_a_storage_error_before_any_row_bytes() {
    let mut dict = two_term_dictionary();
    dict.fail = Some(TermId(1));
    let mut sink = CountingWriter { bytes: Vec::new(), writes: 0, limit: usize::MAX };
    match serialize_ids_to(&many_rows(10), &dict, false, &mut sink) {
        Err(WriteIdsError::Storage(e)) => assert_eq!(e.to_string(), "planted batch failure"),
        other => panic!("expected a storage error, got {other:?}"),
    }
    assert!(!sink.bytes.ends_with(b"]}}"));
}

#[test]
fn a_dictionary_failure_after_the_first_window_leaves_a_body_that_never_ends_in_the_closing_braces() {
    // Rows 0..n-1 use ids 1 and 2; the very last row uses id 3, which the dictionary refuses. The failure therefore falls in a later window than the
    // first, after the head and the first window were already written.
    let mut dict = two_term_dictionary();
    dict.terms.push(Literal::new_simple_literal("third").into());
    dict.fail = Some(TermId(3));
    let n = 200_000;
    let mut ids = many_rows(n).ids;
    *ids.last_mut().unwrap() = TermId(3);
    let sol = IdSolutions { vars: variables(&["x"]), ids, scratch: HashMap::new(), rows: n };
    let mut sink = CountingWriter { bytes: Vec::new(), writes: 0, limit: usize::MAX };
    match serialize_ids_to(&sol, &dict, false, &mut sink) {
        Err(WriteIdsError::Storage(e)) => assert_eq!(e.to_string(), "planted batch failure"),
        other => panic!("expected a storage error, got {other:?}"),
    }
    assert!(sink.bytes.starts_with(br#"{"head":{"vars":["x"]},"results":{"bindings":["#), "the head was written before the failure");
    assert!(sink.bytes.len() > 1_000, "at least one window of rows was written before the failure");
    assert!(!sink.bytes.ends_with(b"]}}"), "a failed stream must never look complete");
    assert!(!sink.bytes.windows(3).any(|w| w == b"]}}"), "the closing braces were never written");
}
