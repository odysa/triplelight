//! Differential bag check for the optional wide-table BGP access path.
use triplelight::exec::{query_with, Options, QueryResult};
use triplelight::storage::wide_tables::{BuildOptions, WithTables};
use triplelight::storage::MemorySnapshot;
use triplelight::storage::{write_disk_snapshot, DiskSnapshot};

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
    assert_eq!(wide.table_count(), 2);
    let query = "SELECT ?s ?p ?q WHERE { ?s <urn:p> ?p . ?s <urn:q> ?q }";
    let expected = bag(query_with(&plain, query, &Options::default()).unwrap());
    let (result, trace) = triplelight::exec::plan_trace::capture(|| {
        query_with(
            &wide,
            query,
            &Options {
                force_wide_tables: true,
                ..Options::default()
            },
        )
        .unwrap()
    });
    assert!(
        trace
            .iter()
            .any(|line| line.contains("strategy=wide-forced")),
        "{trace:?}"
    );
    assert!(
        trace.iter().any(|line| line.contains("access=probe")),
        "the untyped subject requires the index residual: {trace:?}"
    );
    let got = bag(result);
    assert_eq!(got, expected);
    assert_eq!(
        got.len(),
        4,
        "two values in the first table, one in the second, one residual"
    );
}

#[test]
fn date_interval_filters_table_values_before_expansion() {
    const DATE_DATA: &str = r#"
<urn:a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Review> .
<urn:a> <urn:for> <urn:p1> .
<urn:a> <urn:date> "2007-10-01"^^<http://www.w3.org/2001/XMLSchema#date> .
<urn:a> <urn:date> "2007-11-15"^^<http://www.w3.org/2001/XMLSchema#date> .
<urn:b> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Review> .
<urn:b> <urn:for> <urn:p2> .
<urn:b> <urn:date> "2007-11-20"^^<http://www.w3.org/2001/XMLSchema#date> .
<urn:c> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Review> .
<urn:c> <urn:for> <urn:p3> .
<urn:c> <urn:date> "2007-12-01"^^<http://www.w3.org/2001/XMLSchema#date> .
"#;
    let memory = MemorySnapshot::from_ntriples(DATE_DATA.as_bytes()).unwrap();
    let dir = std::env::temp_dir().join(format!("triplelight-wide-date-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write_disk_snapshot(&dir, &memory).unwrap();
    let plain = DiskSnapshot::open(&dir).unwrap();
    let wide = WithTables::build(
        DiskSnapshot::open(&dir).unwrap(),
        BuildOptions {
            min_fill: 0.0,
            min_share: 0.0,
        },
    )
    .unwrap();
    let query = "SELECT ?s ?p ?d WHERE { ?s <urn:for> ?p . ?s <urn:date> ?d . FILTER(?d >= \"2007-11-01\"^^<http://www.w3.org/2001/XMLSchema#date> && ?d < \"2007-11-19\"^^<http://www.w3.org/2001/XMLSchema#date>) }";
    let expected = bag(query_with(&plain, query, &Options::default()).unwrap());
    assert_eq!(expected.len(), 1);
    let (result, trace) = triplelight::exec::plan_trace::capture(|| {
        query_with(
            &wide,
            query,
            &Options {
                force_wide_tables: true,
                ..Options::default()
            },
        )
        .unwrap()
    });
    assert_eq!(bag(result), expected);
    assert!(trace.iter().any(|line| line.contains("table_ranges=1")), "{trace:?}");
    assert!(
        trace
            .iter()
            .any(|line| line.contains("access=wide in=1 out=1")),
        "{trace:?}"
    );
    assert!(
        !trace.iter().any(|line| line.contains("access=probe")),
        "covered subjects need no index residual: {trace:?}"
    );
    let without_range = Options {
        force_wide_tables: true,
        range_pushdown: false,
        ..Options::default()
    };
    assert_eq!(
        bag(query_with(&wide, query, &without_range).unwrap()),
        expected
    );
    drop(wide);
    drop(plain);
    std::fs::remove_dir_all(dir).unwrap();
}
