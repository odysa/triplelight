//! Differential bag check for the optional wide-table BGP access path.
use triplelight::exec::{Options, QueryResult, query_with};
use triplelight::storage::MemorySnapshot;
use triplelight::storage::wide_tables::{BuildOptions, WithTables};

const DATA: &str = r#"
<urn:a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:T1> .
<urn:a> <urn:p> <urn:x> .
<urn:a> <urn:p> <urn:y> .
<urn:a> <urn:q> <urn:d1> .
<urn:b> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:T2> .
<urn:b> <urn:p> <urn:z> .
<urn:b> <urn:q> <urn:d2> .
<urn:c> <urn:p> <urn:w> .
<urn:c> <urn:q> <urn:d3> .
<urn:missing> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:T1> .
<urn:missing> <urn:p> <urn:unused> .
"#;

fn bag(result: QueryResult) -> Vec<String> {
    let QueryResult::Solutions { rows, .. } = result else {
        panic!("expected solutions")
    };
    let mut rows: Vec<String> = rows.into_iter().map(|row| format!("{row:?}")).collect();
    rows.sort();
    rows
}

#[test]
fn forced_table_union_and_residual_equal_plain_bgp_as_bags() {
    let plain = MemorySnapshot::from_ntriples(DATA.as_bytes()).unwrap();
    let indexed = MemorySnapshot::from_ntriples(DATA.as_bytes()).unwrap();
    let wide = WithTables::build(
        indexed,
        BuildOptions {
            min_fill: 0.0,
            min_share: 0.0,
        },
    )
    .unwrap();
    assert_eq!(wide.tables.len(), 2);
    let query = "SELECT ?s ?p ?q WHERE { ?s <urn:p> ?p . ?s <urn:q> ?q }";
    let expected = bag(query_with(&plain, query, &Options::default()).unwrap());
    let (result, trace) = triplelight::exec::plan_trace::capture(|| {
        query_with(&wide, query, &Options { force_wide_tables: true, ..Options::default() }).unwrap()
    });
    assert!(
        trace
            .iter()
            .any(|line| line.contains("strategy=wide-forced")),
        "{trace:?}"
    );
    let got = bag(result);
    assert_eq!(got, expected);
    assert_eq!(
        got.len(),
        4,
        "two values in the first table, one in the second, one residual"
    );
}
