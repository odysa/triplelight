//! SPARQL evaluation through the embedded API: operators and functions, property paths, the optimizer and its switches
//! (range and count pushdown), result formats, SPARQL 1.2 and wide tables. One test target, so the library builds once for all of them.
//! `plan_trace.rs` stays its own target: it needs a process in which `TRIPLELIGHT_TRACE_PLAN` is on before any query runs.
mod count_pushdown;
mod exec;
mod functions;
mod optimizer;
mod paths;
mod range_pushdown;
#[cfg(feature = "rdfs")]
mod rdfs;
mod results;
#[cfg(feature = "rdf-12")]
mod sparql12;
mod wide_table;

#[cfg(not(feature = "rdfs"))]
#[test]
fn rdfs_requires_the_feature() {
    use triplelight::exec::{Entailment, Options, QueryErrorKind, QueryResult, query, query_with};
    use triplelight::storage::MemorySnapshot;

    let snapshot = MemorySnapshot::from_ntriples(&b""[..]).unwrap();
    let ask = "ASK {}";
    assert!(matches!(query(&snapshot, ask), Ok(QueryResult::Boolean(true))));
    let options = Options { entailment: Entailment::Rdfs, ..Options::default() };
    let err = query_with(&snapshot, ask, &options).unwrap_err();
    assert_eq!(err.kind, QueryErrorKind::Unsupported);
    assert!(err.message.contains("rdfs Cargo feature"));
}
