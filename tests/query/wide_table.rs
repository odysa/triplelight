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
    let (auto_result, auto_trace) = triplelight::exec::plan_trace::capture(|| {
        query_with(&wide, query, &Options { auto_wide_tables: true, ..Options::default() }).unwrap()
    });
    assert_eq!(bag(auto_result), expected);
    assert!(auto_trace.iter().any(|line| line.contains("strategy=wide-auto")), "{auto_trace:?}");
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

#[test]
fn numeric_interval_uses_value_order_only_when_ids_prove_it() {
    const YEARS: &str = r#"
<urn:a> <urn:year> "1900"^^<http://www.w3.org/2001/XMLSchema#integer> .
<urn:a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Prize> .
<urn:a> <urn:part> <urn:x> .
<urn:b> <urn:year> "1911"^^<http://www.w3.org/2001/XMLSchema#integer> .
<urn:b> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Prize> .
<urn:b> <urn:part> <urn:y> .
<urn:c> <urn:year> "1912"^^<http://www.w3.org/2001/XMLSchema#integer> .
<urn:c> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Prize> .
<urn:c> <urn:part> <urn:z> .
"#;
    const NONMONOTONE: &str = r#"
<urn:a> <urn:year> "9"^^<http://www.w3.org/2001/XMLSchema#integer> .
<urn:a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Prize> .
<urn:a> <urn:part> <urn:x> .
<urn:b> <urn:year> "10"^^<http://www.w3.org/2001/XMLSchema#integer> .
<urn:b> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Prize> .
<urn:b> <urn:part> <urn:y> .
<urn:c> <urn:year> "11"^^<http://www.w3.org/2001/XMLSchema#integer> .
<urn:c> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Prize> .
<urn:c> <urn:part> <urn:z> .
"#;
    const DECIMAL: &str = r#"
<urn:a> <urn:year> "1909"^^<http://www.w3.org/2001/XMLSchema#integer> .
<urn:a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Prize> .
<urn:a> <urn:part> <urn:x> .
<urn:b> <urn:year> "1910.5"^^<http://www.w3.org/2001/XMLSchema#decimal> .
<urn:b> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Prize> .
<urn:b> <urn:part> <urn:y> .
<urn:c> <urn:year> "1911"^^<http://www.w3.org/2001/XMLSchema#integer> .
<urn:c> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Prize> .
<urn:c> <urn:part> <urn:z> .
"#;
    for (data, query, interval) in [
        (YEARS, "SELECT ?s ?part WHERE { ?s <urn:year> ?year . ?s <urn:part> ?part . FILTER(?year >= 1911 && ?year < 1912) }", true),
        (NONMONOTONE, "SELECT ?s ?part WHERE { ?s <urn:year> ?year . ?s <urn:part> ?part . FILTER(?year >= 10 && ?year < 11) }", false),
        (DECIMAL, "SELECT ?s ?part WHERE { ?s <urn:year> ?year . ?s <urn:part> ?part . FILTER(?year >= 1910.5 && ?year < 1911.0) }", true),
    ] {
        let memory = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
        let dir = std::env::temp_dir().join(format!("triplelight-wide-numeric-{}-{interval}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_disk_snapshot(&dir, &memory).unwrap();
        let plain = DiskSnapshot::open(&dir).unwrap();
        let wide = WithTables::build(DiskSnapshot::open(&dir).unwrap(), BuildOptions { min_fill: 0.0, min_share: 0.0 }).unwrap();
        let expected = bag(query_with(&plain, query, &Options::default()).unwrap());
        assert_eq!(expected.len(), 1);
        let (result, trace) = triplelight::exec::plan_trace::capture(|| {
            query_with(&wide, query, &Options { force_wide_tables: true, ..Options::default() }).unwrap()
        });
        assert_eq!(bag(result), expected);
        assert!(trace.iter().any(|line| line.contains(if interval { "table_ranges=1" } else { "table_ranges=0" })), "{trace:?}");
        drop(wide);
        drop(plain);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn constant_objects_and_embedded_stars_preserve_bags() {
    const MIXED: &str = r#"
<urn:a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:T> .
<urn:a> <urn:p> <urn:x> .
<urn:a> <urn:p> <urn:y> .
<urn:a> <urn:q> <urn:d1> .
<urn:a> <urn:q> <urn:d2> .
<urn:b> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:T> .
<urn:b> <urn:p> <urn:x> .
<urn:c> <urn:p> <urn:x> .
<urn:c> <urn:q> <urn:d3> .
<urn:x> <urn:link> <urn:r1> .
<urn:y> <urn:link> <urn:r2> .
<urn:x> <urn:other> <urn:u1> .
<urn:x> <urn:third> <urn:v1> .
<urn:y> <urn:other> <urn:u2> .
<urn:y> <urn:third> <urn:v2> .
"#;
    let plain = MemorySnapshot::from_ntriples(MIXED.as_bytes()).unwrap();
    let wide = WithTables::build(
        MemorySnapshot::from_ntriples(MIXED.as_bytes()).unwrap(),
        BuildOptions { min_fill: 0.0, min_share: 0.0 },
    ).unwrap();
    let queries = [
        ("SELECT ?s ?q WHERE { ?s <urn:p> <urn:x> . ?s <urn:q> ?q }", 3),
        ("SELECT ?s ?p ?q ?r WHERE { ?s <urn:p> ?p . ?s <urn:q> ?q . ?p <urn:link> ?r }", 5),
        ("SELECT ?s ?p ?q ?r WHERE { ?s <urn:p> ?p . ?s <urn:q> ?q . ?p <urn:link> ?r . FILTER(?r = <urn:r1>) }", 3),
        ("SELECT ?s ?p ?q ?r WHERE { ?p <urn:link> ?r . ?p <urn:other> ?u . ?p <urn:third> ?v . ?s <urn:p> ?p . ?s <urn:q> ?q }", 5),
    ];
    for (query, count) in queries {
        let expected = bag(query_with(&plain, query, &Options::default()).unwrap());
        assert_eq!(expected.len(), count);
        let (result, trace) = triplelight::exec::plan_trace::capture(|| {
            query_with(&wide, query, &Options { force_wide_tables: true, ..Options::default() }).unwrap()
        });
        assert_eq!(bag(result), expected, "{query}");
        assert!(trace.iter().any(|line| line.contains("strategy=wide-forced")), "{query}: {trace:?}");
        let (auto_result, auto_trace) = triplelight::exec::plan_trace::capture(|| {
            query_with(&wide, query, &Options { auto_wide_tables: true, ..Options::default() }).unwrap()
        });
        assert_eq!(bag(auto_result), expected, "{query}");
        assert!(!auto_trace.iter().any(|line| line.contains("strategy=wide-auto")), "{query}: {auto_trace:?}");
    }
}
