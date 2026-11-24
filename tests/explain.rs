#![cfg(unix)]

use triplelight::embedded::Database;
use triplelight::exec::{Entailment, Interrupt, MemoryBudget, Options, QueryErrorKind};

#[test]
fn explain_uses_the_pinned_snapshot_and_keeps_query_results() {
    let db = Database::in_memory();
    db.update("INSERT DATA { <urn:a> <urn:p> <urn:b> }").unwrap();
    let old = db.read();
    db.update("INSERT DATA { <urn:c> <urn:p> <urn:d> }").unwrap();
    let query = "SELECT ?s WHERE { ?s <urn:p> ?o }";

    let before = db.query_json(query).unwrap();
    let old_report = old.explain(query, Entailment::Simple).unwrap();
    let new_report = db.explain(query, Entailment::Simple).unwrap();

    assert!(old_report.contains("patterns=1 in=1 out=1 "), "{old_report}");
    assert!(new_report.contains("patterns=1 in=1 out=2 "), "{new_report}");
    assert_eq!(db.query_json(query).unwrap(), before);
}

#[test]
fn explain_rejects_updates_and_reports_an_empty_plan() {
    let db = Database::in_memory();
    let text = "INSERT DATA { <urn:a> <urn:p> <urn:b> }";
    let snapshot = triplelight::storage::MemorySnapshot::from_ntriples(&b""[..]).unwrap();
    let err = triplelight::exec::plan_trace::explain(&snapshot, text, &Default::default()).unwrap_err();
    assert_eq!(err.kind, QueryErrorKind::Syntax);
    assert!(db.explain("SELECT ?v WHERE { VALUES ?v { 1 } }", Entailment::Simple).unwrap()
        .contains("No basic graph pattern was traced."));
}

#[test]
fn explain_respects_the_query_deadline() {
    let snapshot = triplelight::storage::MemorySnapshot::from_ntriples(&b""[..]).unwrap();
    let interrupt = Interrupt::with_deadline(std::time::Instant::now());
    let options = Options { interrupt: Some(&interrupt), ..Default::default() };
    let error = triplelight::exec::plan_trace::explain(&snapshot, "ASK { ?s ?p ?o }", &options).unwrap_err();
    assert_eq!(error.kind, QueryErrorKind::Timeout);
}

#[test]
fn explain_reports_the_operator_tree_and_request_scoped_phase_totals() {
    let snapshot = triplelight::storage::MemorySnapshot::from_ntriples(&b""[..]).unwrap();
    let query = "SELECT ?bucket (COUNT(*) AS ?n) WHERE { VALUES ?x { 1 2 3 } BIND((?x + 1) AS ?bucket) FILTER(?bucket > 1) } GROUP BY ?bucket ORDER BY DESC(?n)";
    let report = triplelight::exec::plan_trace::explain(&snapshot, query, &Default::default()).unwrap();
    for expected in ["Observed operator tree", "OrderBy keys=1", "Group keys=1 aggregates=1", "Filter", "Extend ?bucket", "Values variables=1 rows=3"] {
        assert!(report.contains(expected), "missing {expected:?}:\n{report}");
    }
    assert!(report.contains("bindings_in=") && report.contains("calls=1"), "{report}");
    assert!(report.contains("Phase timings (microseconds; nested work may overlap)"), "{report}");
    for phase in ["parse_us=", "bgp_us=", "group_us=", "count_us=", "sort_us=", "filter_us=", "extend_us=", "join_us="] {
        assert!(report.contains(phase), "missing {phase}:\n{report}");
    }
}

fn phase(report: &str, name: &str) -> u64 {
    report.split_whitespace().find_map(|field| field.strip_prefix(name)?.parse().ok()).unwrap()
}

#[test]
fn pushed_filter_stays_in_the_existing_bgp_phase() {
    let data = (0..100).map(|n| format!("<urn:s{n}> <urn:p> \"{n}\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n")).collect::<String>();
    let snapshot = triplelight::storage::MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let report = triplelight::exec::plan_trace::explain(&snapshot, "SELECT ?s WHERE { ?s <urn:p> ?n FILTER(?n > 50) }", &Default::default()).unwrap();
    assert!(phase(&report, "bgp_us=") > 0, "{report}");
    assert_eq!(phase(&report, "filter_us="), 0, "pushed filter was counted twice:\n{report}");
}

#[test]
fn failed_bgp_restores_phase_depth_for_the_next_explain() {
    let data = (0..5_000).map(|n| format!("<urn:s{n}> <urn:p> \"{n}\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n")).collect::<String>();
    let snapshot = triplelight::storage::MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let budget = MemoryBudget::new(1);
    let options = Options { memory_budget: Some(&budget), ..Default::default() };
    let stopped = triplelight::exec::plan_trace::explain(&snapshot, "SELECT ?s WHERE { ?s <urn:p> ?n }", &options);
    assert_eq!(stopped.unwrap_err().kind, QueryErrorKind::ResourceLimit);
    let report = triplelight::exec::plan_trace::explain(&snapshot, "SELECT ?s WHERE { ?s <urn:p> ?n FILTER(?n > 2500) }", &Default::default()).unwrap();
    assert!(phase(&report, "bgp_us=") > 0, "stale BGP depth suppressed the next capture:\n{report}");
    assert_eq!(phase(&report, "filter_us="), 0, "{report}");
}

#[test]
fn repeated_optional_child_is_folded_by_algebra_identity() {
    let snapshot = triplelight::storage::MemorySnapshot::from_ntriples(b"<urn:s> <urn:p> <urn:o> .\n".as_slice()).unwrap();
    let values = std::iter::repeat_n("<urn:s>", 1_000).collect::<Vec<_>>().join(" ");
    let query = format!("SELECT ?s ?o WHERE {{ VALUES ?s {{ {values} }} OPTIONAL {{ ?s <urn:p> ?o }} }}");
    let report = triplelight::exec::plan_trace::explain(&snapshot, &query, &Default::default()).unwrap();
    assert!(report.contains("BGP patterns=1 calls=1000"), "{report}");
    assert!(!report.contains("Operator tree truncated."), "{report}");
}

#[test]
fn operator_tree_is_bounded_without_stopping_the_query() {
    let snapshot = triplelight::storage::MemorySnapshot::from_ntriples(&b""[..]).unwrap();
    let arms = (0..520).map(|n| format!("{{ VALUES ?x {{ {n} }} }}")).collect::<Vec<_>>().join(" UNION ");
    let report = triplelight::exec::plan_trace::explain(&snapshot, &format!("SELECT ?x WHERE {{ {arms} }}"), &Default::default()).unwrap();
    assert!(report.contains("Operator tree truncated."), "{report}");
    assert!(report.len() < 2 * 256 * 1024, "bounded report grew to {} bytes", report.len());
}
