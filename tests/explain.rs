#![cfg(unix)]

use triplelight::embedded::Database;
use triplelight::exec::{Entailment, Interrupt, Options, QueryErrorKind};

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
