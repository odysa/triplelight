//! `TRIPLELIGHT_TRACE_PLAN` (`exec::plan_trace`). The flag is read once per process, so this file is its own test
//! binary and turns the trace on before any query runs; keep further tests that need it off elsewhere.
use triplelight::exec::{plan_trace, query, QueryResult};
use triplelight::storage::MemorySnapshot;

const DATA: &str = r#"
<http://ex/alice> <http://ex/knows> <http://ex/bob> .
<http://ex/alice> <http://ex/knows> <http://ex/carol> .
<http://ex/bob> <http://ex/knows> <http://ex/carol> .
<http://ex/carol> <http://ex/knows> <http://ex/alice> .
<http://ex/alice> <http://ex/age> "30"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://ex/bob> <http://ex/age> "25"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://ex/carol> <http://ex/age> "41"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://ex/alice> <http://ex/name> "Alice" .
<http://ex/bob> <http://ex/name> "Bob"@en .
"#;

#[test]
fn three_pattern_bgp_prints_one_plan_line() {
    assert!(plan_trace::force_on());
    let store = MemorySnapshot::from_ntriples(DATA.as_bytes()).unwrap();
    let q = "SELECT ?a ?n WHERE { ?a <http://ex/knows> ?b . ?b <http://ex/age> ?age . ?b <http://ex/name> ?n }";
    let (result, lines) = plan_trace::capture(|| query(&store, q).unwrap());
    let QueryResult::Solutions { rows, .. } = result else { panic!("not solutions") };
    // alice -> bob ("Bob"), carol -> alice ("Alice"); carol has no name.
    assert_eq!(rows.len(), 2);
    assert_eq!(lines.len(), 1, "{lines:?}");
    let line = &lines[0];
    assert!(line.starts_with("plan_trace strategy=greedy patterns=3 in=1 out=2 "), "{line}");
    assert_eq!(line.matches(" | #").count(), 3, "one step per pattern: {line}");
    assert!(line.contains("[?a knows ?b]") && line.contains("[?b age ?age]") && line.contains("[?b name ?n]"), "{line}");
    assert!(line.contains(" access=") && line.contains(" us="), "{line}");
}
