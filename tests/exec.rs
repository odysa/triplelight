use rdfdb::exec::{query, QueryResult};
use rdfdb::storage::MemorySnapshot;

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

fn store() -> MemorySnapshot { MemorySnapshot::from_ntriples(DATA.as_bytes()).unwrap() }

fn rows(q: &str) -> Vec<Vec<String>> {
    match query(&store(), q).unwrap() {
        QueryResult::Solutions { rows, .. } => {
            let mut v: Vec<Vec<String>> = rows.into_iter().map(|r| r.into_iter().map(|t| t.map(|t| t.to_string()).unwrap_or("-".into())).collect()).collect();
            v.sort();
            v
        }
        r => panic!("{r:?}"),
    }
}

#[test]
fn bgp_star_and_chain() {
    assert_eq!(rows("SELECT ?x WHERE { ?x <http://ex/knows> <http://ex/carol> }").len(), 2);
    // triangle: alice -> bob -> carol -> alice (and rotations)
    let tri = rows("SELECT ?a ?b ?c WHERE { ?a <http://ex/knows> ?b . ?b <http://ex/knows> ?c . ?c <http://ex/knows> ?a }");
    assert_eq!(tri.len(), 3);
}

#[test]
fn filter_optional_order_limit() {
    let r = rows("SELECT ?x ?n WHERE { ?x <http://ex/age> ?a OPTIONAL { ?x <http://ex/name> ?n } FILTER(?a > 26) }");
    assert_eq!(r, vec![vec!["<http://ex/alice>".to_string(), "\"Alice\"".into()], vec!["<http://ex/carol>".into(), "-".into()]]);
    match query(&store(), "SELECT ?x WHERE { ?x <http://ex/age> ?a } ORDER BY DESC(?a) LIMIT 1").unwrap() {
        QueryResult::Solutions { rows, .. } => assert_eq!(rows[0][0].as_ref().unwrap().to_string(), "<http://ex/carol>"),
        _ => unreachable!(),
    }
}

#[test]
fn union_distinct_group_ask() {
    assert_eq!(rows("SELECT DISTINCT ?x WHERE { { ?x <http://ex/knows> ?y } UNION { ?y <http://ex/knows> ?x } }").len(), 3);
    let g = rows("SELECT ?x (COUNT(?y) AS ?n) WHERE { ?x <http://ex/knows> ?y } GROUP BY ?x");
    assert!(g.contains(&vec!["<http://ex/alice>".into(), "\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>".into()]));
    assert!(matches!(query(&store(), "ASK { <http://ex/bob> <http://ex/knows> <http://ex/alice> }").unwrap(), QueryResult::Boolean(false)));
    assert!(matches!(query(&store(), "ASK { <http://ex/nobody> ?p ?o }").unwrap(), QueryResult::Boolean(false)));
    let l = rows("SELECT ?n WHERE { ?x <http://ex/name> ?n FILTER(lang(?n) = \"en\") }");
    assert_eq!(l.len(), 1);
    let b = rows("SELECT ?x ?d WHERE { ?x <http://ex/age> ?a BIND(?a * 2 AS ?d) FILTER(?d < 55) }");
    assert_eq!(b.len(), 1);
}

#[test]
fn leapfrog_matches_bind_join() {
    let q = "SELECT ?a ?b ?c WHERE { ?a <http://ex/knows> ?b . ?b <http://ex/knows> ?c . ?c <http://ex/knows> ?a }";
    let run = |wcoj| match rdfdb::exec::query_with(&store(), q, &rdfdb::exec::Options { stats: None, wcoj, raw_ids: false }).unwrap() {
        QueryResult::Solutions { rows, .. } => { let mut v: Vec<String> = rows.iter().map(|r| format!("{r:?}")).collect(); v.sort(); v }
        _ => unreachable!(),
    };
    let (off, on) = (run(false), run(true));
    assert_eq!(off, on);
    assert_eq!(on.len(), 3);
}

#[test]
fn id_results_serialize_like_decoded() {
    let s = store();
    let q = "SELECT ?x ?n ?d WHERE { ?x <http://ex/age> ?a OPTIONAL { ?x <http://ex/name> ?n } BIND(?a * 2 AS ?d) }";
    let opts = rdfdb::exec::Options { raw_ids: true, ..Default::default() };
    let QueryResult::Ids(sol) = rdfdb::exec::query_with(&s, q, &opts).unwrap() else { panic!() };
    let fast = rdfdb::exec::results::serialize_ids(&sol, &s).unwrap();
    let slow = rdfdb::exec::results::serialize(&query(&s, q).unwrap()).1;
    assert_eq!(fast, slow);
}

#[test]
fn errors_carry_their_kind() {
    use rdfdb::exec::QueryErrorKind;
    assert_eq!(query(&store(), "SELECT WHERE {").unwrap_err().kind, QueryErrorKind::Syntax);
    assert_eq!(query(&store(), "DESCRIBE <http://ex/alice>").unwrap_err().kind, QueryErrorKind::Unsupported);
}

/// A dictionary failure during query evaluation is a Storage error, not a missing result.
#[test]
fn dictionary_errors_are_storage_errors() {
    use rdfdb::model::{Perm, TermId, TripleBatch};
    use rdfdb::storage::{Dictionary, Snapshot, StorageError};
    struct BadDict(MemorySnapshot);
    impl Dictionary for BadDict {
        fn lookup(&self, _: &oxrdf::Term) -> Result<Option<TermId>, StorageError> { Err(StorageError::other("planted dictionary failure")) }
        fn resolve(&self, id: TermId) -> Result<oxrdf::Term, StorageError> { self.0.resolve(id) }
        fn with_term(&self, id: TermId, f: &mut dyn FnMut(oxrdf::TermRef<'_>)) -> Result<(), StorageError> { self.0.with_term(id, f) }
    }
    impl Snapshot for BadDict {
        fn scan<'a>(&'a self, p: Perm, pre: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> { self.0.scan(p, pre) }
        fn count(&self, p: Perm, pre: &[TermId]) -> Result<u64, StorageError> { self.0.count(p, pre) }
        fn seek(&self, p: Perm, pre: &[TermId], t: TermId) -> Result<Option<TermId>, StorageError> { self.0.seek(p, pre, t) }
    }
    let e = query(&BadDict(store()), "SELECT ?x WHERE { ?x <http://ex/knows> <http://ex/bob> }").unwrap_err();
    assert_eq!(e.kind, rdfdb::exec::QueryErrorKind::Storage);
}

/// Filters pushed into a BGP give the same answers as filtering afterwards (OPTIONAL blocks pushdown).
#[test]
fn filter_pushdown_matches_post_filtering() {
    let pushed = rows("SELECT ?x ?y WHERE { ?x <http://ex/knows> ?y . ?x <http://ex/age> ?a . ?y <http://ex/age> ?b FILTER(?a > ?b && ?b > 20) }");
    let post = rows("SELECT ?x ?y WHERE { { ?x <http://ex/knows> ?y . ?x <http://ex/age> ?a . ?y <http://ex/age> ?b } OPTIONAL { ?x <http://ex/nothing> ?z } FILTER(?a > ?b && ?b > 20) }");
    assert_eq!(pushed, post);
    assert_eq!(pushed.len(), 2); // alice(30)->bob(25), carol(41)->alice(30)
    // A conjunct over a variable outside the BGP stays a post-filter and keeps its semantics (unbound -> false).
    assert!(rows("SELECT ?x WHERE { ?x <http://ex/age> ?a FILTER(?a > 20 && BOUND(?nope)) }").is_empty());
}

/// SELECT ?x ... ORDER BY ... LIMIT: the result header lists only the projected variables.
#[test]
fn projection_under_slice_and_distinct() {
    match query(&store(), "SELECT DISTINCT ?x WHERE { ?x <http://ex/age> ?a FILTER(?a > 26) } ORDER BY ?a LIMIT 5").unwrap() {
        QueryResult::Solutions { vars, rows } => { assert_eq!(vars.len(), 1); assert_eq!(vars[0].as_str(), "x"); assert_eq!(rows.len(), 2) }
        _ => unreachable!(),
    }
}

/// The compiled numeric FILTER path (pushed into the BGP) must agree with the general evaluator (post-filter over an
/// OPTIONAL) on mixed numeric types, non-numeric values (fallback), integer overflow (error) and three-valued && / || / !.
#[test]
fn numeric_filter_fast_path_matches_general_evaluator() {
    let data = r#"
<http://ex/a> <http://ex/v> "5"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://ex/b> <http://ex/v> "5.5"^^<http://www.w3.org/2001/XMLSchema#decimal> .
<http://ex/c> <http://ex/v> "6.0E0"^^<http://www.w3.org/2001/XMLSchema#double> .
<http://ex/d> <http://ex/v> "abc" .
<http://ex/e> <http://ex/v> "9223372036854775807"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://ex/f> <http://ex/v> "-3"^^<http://www.w3.org/2001/XMLSchema#int> .
<http://ex/g> <http://ex/v> "abd" .
"#;
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let run = |q: &str| -> Vec<String> {
        match query(&s, q).unwrap() {
            QueryResult::Solutions { rows, .. } => { let mut v: Vec<String> = rows.into_iter().map(|r| format!("{r:?}")).collect(); v.sort(); v }
            r => panic!("{r:?}"),
        }
    };
    for f in ["?v > 5", "?v >= 5", "?v < ?w + 1", "?v * 2 > ?w - 1", "?v + 1 > 0", "!(?v < 5.5)", "?v < 5 || ?w > 5", "?v > 0 && ?w < 6",
              "?v > \"abb\"", "?v - ?w <= 0", "!(?v + 1 > ?w)"] {
        let pushed = run(&format!("SELECT ?x ?y WHERE {{ ?x <http://ex/v> ?v . ?y <http://ex/v> ?w FILTER({f}) }}"));
        let post = run(&format!("SELECT ?x ?y WHERE {{ {{ ?x <http://ex/v> ?v . ?y <http://ex/v> ?w }} OPTIONAL {{ ?x <http://ex/none> ?z }} FILTER({f}) }}"));
        assert_eq!(pushed, post, "filter {f}");
    }
    // Overflow is an error (row dropped) on both paths; "abc" > "abb" is a string comparison via the fallback.
    assert_eq!(run("SELECT ?x WHERE { ?x <http://ex/v> ?v FILTER(?v + 1 > 0) }").len(), 3); // a, b, c (e overflows, f = -3)
    assert_eq!(run("SELECT ?x ?y WHERE { ?x <http://ex/v> ?v . ?y <http://ex/v> ?w FILTER(?v > \"abb\") }").len(), 14); // ?x in {abc, abd} x 7
}

/// Integer comparison is exact beyond 2^53 on both the compiled and the general path.
#[test]
fn integer_comparison_exact_above_2_pow_53() {
    let data = r#"
<http://ex/a> <http://ex/v> "9007199254740993"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://ex/b> <http://ex/v> "9007199254740992"^^<http://www.w3.org/2001/XMLSchema#integer> .
"#;
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let n = |q: &str| match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => rows.len(), r => panic!("{r:?}") };
    let fast = "SELECT ?x ?y WHERE { ?x <http://ex/v> ?v . ?y <http://ex/v> ?w FILTER(?v > ?w) }";
    let slow = "SELECT ?x ?y WHERE { { ?x <http://ex/v> ?v . ?y <http://ex/v> ?w } OPTIONAL { ?x <http://ex/none> ?z } FILTER(?v > ?w) }";
    assert_eq!(n(fast), 1);
    assert_eq!(n(slow), 1);
    assert_eq!(n("SELECT ?x WHERE { ?x <http://ex/v> ?v FILTER(?v = 9007199254740992) }"), 1);
}

/// `?x = <iri>` / `!=` / sameTerm pushed into the BGP (ID comparison) agree with the general evaluator, including
/// literal-valued bindings and an IRI absent from the store.
#[test]
fn iri_equality_filter_fast_path_matches_general_evaluator() {
    for f in ["?y = <http://ex/bob>", "?y != <http://ex/bob>", "<http://ex/bob> != ?y", "sameTerm(?y, <http://ex/carol>)",
              "?y != <http://ex/nobody>", "?y = <http://ex/nobody>", "?n != <http://ex/bob>", "?n = <http://ex/bob>"] {
        let pushed = rows(&format!("SELECT ?x ?y ?n WHERE {{ ?x <http://ex/knows> ?y . ?x <http://ex/name> ?n FILTER({f}) }}"));
        let post = rows(&format!("SELECT ?x ?y ?n WHERE {{ {{ ?x <http://ex/knows> ?y . ?x <http://ex/name> ?n }} OPTIONAL {{ ?x <http://ex/none> ?z }} FILTER({f}) }}"));
        assert_eq!(pushed, post, "filter {f}");
    }
    assert_eq!(rows("SELECT ?x WHERE { ?x <http://ex/knows> ?y FILTER(?y != <http://ex/carol>) }").len(), 2);
}

/// ORDER BY over an XSD cast (BSBM q023: `ORDER BY xsd:double(str(?price))`) sorts by the cast value; casts follow
/// SPARQL 1.1 §17.5 (unparsable lexical forms are errors, which sort first as unbound).
#[test]
fn order_by_xsd_cast_and_casts() {
    let data = r#"
<http://ex/a> <http://ex/price> "7851.30"^^<http://www.w3.org/2001/XMLSchema#decimal> .
<http://ex/b> <http://ex/price> "2216.71"^^<http://www.w3.org/2001/XMLSchema#decimal> .
<http://ex/c> <http://ex/price> "5947.37"^^<http://www.w3.org/2001/XMLSchema#decimal> .
<http://ex/d> <http://ex/price> "10.5" .
"#;
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let ordered = |q: &str| -> Vec<String> {
        match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => rows.into_iter().map(|r| r[0].as_ref().unwrap().to_string()).collect(), r => panic!("{r:?}") }
    };
    let q = "PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> SELECT ?s WHERE { ?s <http://ex/price> ?p } ORDER BY xsd:double(str(?p))";
    assert_eq!(ordered(q), vec!["<http://ex/d>", "<http://ex/b>", "<http://ex/c>", "<http://ex/a>"]);
    let desc = "PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> SELECT ?s WHERE { ?s <http://ex/price> ?p } ORDER BY DESC(xsd:double(?p)) LIMIT 1";
    assert_eq!(ordered(desc), vec!["<http://ex/a>"]);
    let one = |e: &str| -> String {
        match query(&s, &format!("PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> SELECT ?v WHERE {{ BIND({e} AS ?v) }}")).unwrap() {
            QueryResult::Solutions { rows, .. } => rows[0][0].as_ref().map_or("-".into(), |t| t.to_string()), r => panic!("{r:?}") }
    };
    assert_eq!(one("xsd:integer(\"42\")"), "\"42\"^^<http://www.w3.org/2001/XMLSchema#integer>");
    assert_eq!(one("xsd:integer(3.9)"), "\"3\"^^<http://www.w3.org/2001/XMLSchema#integer>");
    assert_eq!(one("xsd:boolean(\"1\")"), "\"true\"^^<http://www.w3.org/2001/XMLSchema#boolean>");
    assert_eq!(one("xsd:string(<http://ex/a>)"), "\"http://ex/a\"");
    assert_eq!(one("xsd:double(\"abc\")"), "-"); // cast error -> unbound
}

/// The ID-level hash aggregation agrees with the general GROUP BY evaluator on typing, errors, ties and DISTINCT.
#[test]
fn fast_group_matches_general_group() {
    let data = r#"
<http://ex/a> <http://ex/g> "x" .
<http://ex/a> <http://ex/v> "1"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://ex/b> <http://ex/g> "x" .
<http://ex/b> <http://ex/v> "2.5"^^<http://www.w3.org/2001/XMLSchema#decimal> .
<http://ex/c> <http://ex/g> "y" .
<http://ex/c> <http://ex/v> "3"^^<http://www.w3.org/2001/XMLSchema#int> .
<http://ex/d> <http://ex/g> "y" .
<http://ex/d> <http://ex/v> "03"^^<http://www.w3.org/2001/XMLSchema#int> .
<http://ex/e> <http://ex/g> "z" .
<http://ex/e> <http://ex/v> "abc" .
<http://ex/f> <http://ex/g> "z" .
<http://ex/f> <http://ex/v> "1.0E0"^^<http://www.w3.org/2001/XMLSchema#double> .
<http://ex/h> <http://ex/g> "w" .
<http://ex/h> <http://ex/v> "9223372036854775807"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://ex/i> <http://ex/g> "w" .
<http://ex/i> <http://ex/v> "1"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://ex/j> <http://ex/g> "x" .
"#;
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let run = |q: &str| -> Vec<String> {
        match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => rows.into_iter().map(|r| format!("{r:?}")).collect(), r => panic!("{r:?}") }
    };
    let queries = [
        "SELECT ?g (COUNT(*) AS ?n) (COUNT(?v) AS ?c) (SUM(?v) AS ?s) (AVG(?v) AS ?a) WHERE { ?x <http://ex/g> ?g OPTIONAL { ?x <http://ex/v> ?v } } GROUP BY ?g",
        "SELECT ?g (MIN(?v) AS ?lo) (MAX(?v) AS ?hi) (SAMPLE(?v) AS ?sm) WHERE { ?x <http://ex/g> ?g OPTIONAL { ?x <http://ex/v> ?v } } GROUP BY ?g",
        "SELECT (COUNT(DISTINCT ?g) AS ?n) (SUM(DISTINCT ?v) AS ?s) (COUNT(DISTINCT *) AS ?d) WHERE { ?x <http://ex/g> ?g OPTIONAL { ?x <http://ex/v> ?v } }",
        "SELECT (COUNT(*) AS ?n) (SUM(?v) AS ?s) (AVG(?v) AS ?a) WHERE { ?x <http://ex/none> ?v }",
        "SELECT ?g (AVG(?v) AS ?a) WHERE { ?x <http://ex/g> ?g OPTIONAL { ?x <http://ex/v> ?v } } GROUP BY ?g HAVING (COUNT(?v) > 1)",
    ];
    let fast: Vec<_> = queries.iter().map(|q| run(q)).collect();
    // SAFETY: test-local toggle; other tests merely take the (equivalent) general path while it is set.
    unsafe { std::env::set_var("RDFDB_LEGACY_GROUP", "1") };
    let legacy: Vec<_> = queries.iter().map(|q| run(q)).collect();
    unsafe { std::env::remove_var("RDFDB_LEGACY_GROUP") };
    for ((q, f), l) in queries.iter().zip(&fast).zip(&legacy) { assert_eq!(f, l, "{q}"); }
}

/// xsd:float is its own type in numeric promotion (integer < decimal < float < double), not folded into double.
#[test]
fn float_promotion_and_aggregates() {
    let data = r#"
<http://ex/a> <http://ex/f> "1.5"^^<http://www.w3.org/2001/XMLSchema#float> .
<http://ex/b> <http://ex/f> "2.25"^^<http://www.w3.org/2001/XMLSchema#float> .
<http://ex/a> <http://ex/d> "1.0E0"^^<http://www.w3.org/2001/XMLSchema#double> .
"#;
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let dt = |q: &str| -> String {
        match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => match rows[0][0].as_ref().unwrap() {
            oxrdf::Term::Literal(l) => l.datatype().as_str().rsplit('#').next().unwrap().to_string(), t => panic!("{t}") }, r => panic!("{r:?}") }
    };
    assert_eq!(dt("SELECT ?v WHERE { <http://ex/a> <http://ex/f> ?f BIND(?f + 1 AS ?v) }"), "float");
    assert_eq!(dt("SELECT ?v WHERE { <http://ex/a> <http://ex/f> ?f BIND(?f + 1.5 AS ?v) }"), "float");
    assert_eq!(dt("SELECT ?v WHERE { <http://ex/a> <http://ex/f> ?f . <http://ex/a> <http://ex/d> ?d BIND(?f * ?d AS ?v) }"), "double");
    assert_eq!(dt("SELECT (SUM(?f) AS ?v) WHERE { ?x <http://ex/f> ?f }"), "float");
    assert_eq!(dt("SELECT (AVG(?f) AS ?v) WHERE { ?x <http://ex/f> ?f }"), "float");
    assert_eq!(dt("PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> SELECT ?v WHERE { BIND(xsd:float(\"0.1\") AS ?v) }"), "float");
    // Lexical form as Jena writes computed floats (Java Float.toString).
    match query(&s, "SELECT (SUM(?f) AS ?v) WHERE { ?x <http://ex/f> ?f }").unwrap() {
        QueryResult::Solutions { rows, .. } => assert_eq!(rows[0][0].as_ref().unwrap().to_string(), "\"3.75\"^^<http://www.w3.org/2001/XMLSchema#float>"),
        r => panic!("{r:?}"),
    }
    match query(&s, "SELECT ?v WHERE { <http://ex/a> <http://ex/f> ?f BIND(?f * 4 AS ?v) }").unwrap() {
        QueryResult::Solutions { rows, .. } => assert_eq!(rows[0][0].as_ref().unwrap().to_string(), "\"6.0\"^^<http://www.w3.org/2001/XMLSchema#float>"),
        r => panic!("{r:?}"),
    }
}

/// Aggregate errors follow SPARQL 1.1 §18.5 (and Jena): an error or unbound value in the group makes
/// SUM/AVG/MIN/MAX/GROUP_CONCAT unbound; COUNT(expr) counts non-error values; SAMPLE returns a non-error value.
#[test]
fn aggregate_errors_poison_like_jena() {
    let data = r#"
<urn:a> <urn:g> "x" .
<urn:a> <urn:v> "1"^^<http://www.w3.org/2001/XMLSchema#integer> .
<urn:b> <urn:g> "x" .
<urn:c> <urn:g> "x" .
<urn:c> <urn:v> "bad" .
<urn:d> <urn:g> "y" .
<urn:d> <urn:v> "2"^^<http://www.w3.org/2001/XMLSchema#integer> .
<urn:e> <urn:g> "y" .
<urn:f> <urn:g> "z" .
<urn:f> <urn:v> "3"^^<http://www.w3.org/2001/XMLSchema#integer> .
"#;
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let rows = |q: &str| -> Vec<Vec<String>> {
        match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => {
            let mut v: Vec<Vec<String>> = rows.into_iter().map(|r| r.into_iter().map(|t| t.map_or("-".into(), |t| t.to_string())).collect()).collect(); v.sort(); v }
            r => panic!("{r:?}") }
    };
    let int = |n: i64| format!("\"{n}\"^^<http://www.w3.org/2001/XMLSchema#integer>");
    let q1 = "SELECT ?g (SUM(?v) AS ?s) (COUNT(?v) AS ?c) (MIN(?v) AS ?mn) (MAX(?v) AS ?mx) (AVG(?v) AS ?a) WHERE { ?x <urn:g> ?g OPTIONAL { ?x <urn:v> ?v } } GROUP BY ?g";
    let q2 = "SELECT ?g (SUM(?v*2) AS ?s) (COUNT(?v*2) AS ?c) (MIN(?v*2) AS ?mn) (GROUP_CONCAT(?v*2) AS ?gc) WHERE { ?x <urn:g> ?g OPTIONAL { ?x <urn:v> ?v } } GROUP BY ?g";
    for legacy in [false, true] {
        // SAFETY: test-local toggle; other tests just take the equivalent general path while set.
        if legacy { unsafe { std::env::set_var("RDFDB_LEGACY_GROUP", "1") } }
        assert_eq!(rows(q1), vec![
            vec!["\"x\"".into(), "-".into(), int(2), "-".into(), "-".into(), "-".into()],
            vec!["\"y\"".into(), "-".into(), int(1), "-".into(), "-".into(), "-".into()],
            vec!["\"z\"".into(), int(3), int(1), int(3), int(3), "\"3.0\"^^<http://www.w3.org/2001/XMLSchema#decimal>".into()],
        ], "legacy={legacy}");
        assert_eq!(rows(q2), vec![
            vec!["\"x\"".into(), "-".into(), int(1), "-".into(), "-".into()],
            vec!["\"y\"".into(), "-".into(), int(1), "-".into(), "-".into()],
            vec!["\"z\"".into(), int(6), int(1), int(6), "\"6\"".into()],
        ], "legacy={legacy}");
        if legacy { unsafe { std::env::remove_var("RDFDB_LEGACY_GROUP") } }
    }
}

/// Float SUM accumulates in f64 and rounds to binary32 once: 2^24 + 1 + 1 (each 1 alone would be lost to per-step
/// binary32 rounding) sums to the binary32 value nearest the exact 16777218.
#[test]
fn float_sum_rounds_once() {
    let data = r#"
<urn:a> <urn:f> "16777216"^^<http://www.w3.org/2001/XMLSchema#float> .
<urn:b> <urn:f> "1"^^<http://www.w3.org/2001/XMLSchema#float> .
<urn:c> <urn:f> "1.0"^^<http://www.w3.org/2001/XMLSchema#float> .
"#;
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    match query(&s, "SELECT (SUM(?f) AS ?v) WHERE { ?x <urn:f> ?f }").unwrap() {
        QueryResult::Solutions { rows, .. } => assert_eq!(rows[0][0].as_ref().unwrap().to_string(), "\"1.6777218E7\"^^<http://www.w3.org/2001/XMLSchema#float>"),
        r => panic!("{r:?}"),
    }
}

/// Expression arithmetic still rounds each float operation (only SUM/AVG accumulate wider): 2^24 + 1 in binary32 is
/// 2^24, so this filter rejects the row on both the compiled and the general path.
#[test]
fn float_expression_rounds_per_operation() {
    let data = r#"
<urn:r> <urn:a> "16777216"^^<http://www.w3.org/2001/XMLSchema#float> .
<urn:r> <urn:b> "1"^^<http://www.w3.org/2001/XMLSchema#float> .
"#;
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let n = |q: &str| match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => rows.len(), r => panic!("{r:?}") };
    let f = "FILTER((?a + ?b) > \"16777216\"^^<http://www.w3.org/2001/XMLSchema#float>)";
    assert_eq!(n(&format!("SELECT * WHERE {{ ?r <urn:a> ?a . ?r <urn:b> ?b {f} }}")), 0);
    assert_eq!(n(&format!("SELECT * WHERE {{ {{ ?r <urn:a> ?a . ?r <urn:b> ?b }} OPTIONAL {{ ?r <urn:none> ?z }} {f} }}")), 0);
}

/// ORDER BY ... LIMIT uses a stable top-k: same rows and order as sorting everything, including ties and OFFSET.
#[test]
fn order_by_limit_top_k_matches_full_sort() {
    let mut data = String::new();
    for i in 0..200 { data.push_str(&format!("<urn:s{i}> <urn:p> \"{}\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n", (i * 37) % 23)); }
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let seq = |q: &str| -> Vec<String> {
        match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => rows.into_iter().map(|r| format!("{r:?}")).collect(), r => panic!("{r:?}") }
    };
    for (order, lim) in [("?o", "LIMIT 7"), ("DESC(?o)", "LIMIT 10 OFFSET 5"), ("?o ?s", "LIMIT 1"), ("DESC(?o) ?s", "LIMIT 300"), ("?o", "LIMIT 0")] {
        let full = seq(&format!("SELECT ?s ?o WHERE {{ ?s <urn:p> ?o }} ORDER BY {order}"));
        let topk = seq(&format!("SELECT ?s ?o WHERE {{ ?s <urn:p> ?o }} ORDER BY {order} {lim}"));
        let (off, n) = match lim.split_whitespace().collect::<Vec<_>>()[..] {
            ["LIMIT", n] => (0, n.parse::<usize>().unwrap()), ["LIMIT", n, "OFFSET", o] => (o.parse().unwrap(), n.parse().unwrap()), _ => unreachable!() };
        assert_eq!(topk, full.into_iter().skip(off).take(n).collect::<Vec<_>>(), "{order} {lim}");
    }
}

/// `FILTER(?a = ?b)` between separate patterns is evaluated as a join when neither side can be a literal, with the
/// same answers as the general filter; literal-valued variables keep value equality ("1"^^int = "01"^^int).
#[test]
fn variable_equality_unification_matches_general_filter() {
    let data = r#"
<urn:p1> <urn:feat> <urn:f1> .
<urn:p1> <urn:feat> <urn:f2> .
<urn:p2> <urn:feat> <urn:f1> .
<urn:p2> <urn:feat> <urn:f3> .
<urn:p3> <urn:feat> _:b .
<urn:p4> <urn:feat> _:b .
<urn:p1> <urn:num> "1"^^<http://www.w3.org/2001/XMLSchema#int> .
<urn:p2> <urn:num> "01"^^<http://www.w3.org/2001/XMLSchema#int> .
"#;
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let rows = |q: &str| -> Vec<String> {
        match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => { let mut v: Vec<String> = rows.into_iter().map(|r| format!("{r:?}")).collect(); v.sort(); v } r => panic!("{r:?}") }
    };
    for (p, cmp) in [("<urn:feat>", "?f = ?g"), ("<urn:feat>", "sameTerm(?f, ?g)"), ("<urn:num>", "?f = ?g")] {
        let fast = rows(&format!("SELECT ?x ?y ?f ?g WHERE {{ ?x {p} ?f . ?y {p} ?g FILTER({cmp}) }}"));
        let general = rows(&format!("SELECT ?x ?y ?f ?g WHERE {{ ?x {p} ?f . ?y {p} ?g FILTER({cmp} || false) }}"));
        assert_eq!(fast, general, "{p} {cmp}");
    }
    // The literal case keeps value equality: p1 and p2 match each other through "1" = "01".
    assert_eq!(rows("SELECT ?x ?y WHERE { ?x <urn:num> ?f . ?y <urn:num> ?g FILTER(?f = ?g) }").len(), 4);
    // Grouped count over the unified join (the BSBM-BI q2 shape).
    let q2 = "SELECT ?y (COUNT(?g) AS ?n) WHERE { <urn:p1> <urn:feat> ?f . ?y <urn:feat> ?g FILTER(?f = ?g) } GROUP BY ?y";
    assert_eq!(rows(q2).len(), 2);
}

/// The equality rewrite must not drop an outer binding of either variable (Jianlin's counterexample).
#[test]
fn variable_equality_unification_respects_bound_inputs() {
    let data = r#"
<http://ex/s1> <http://ex/p> <http://ex/A> .
<http://ex/s2> <http://ex/q> <http://ex/A> .
<http://ex/s3> <http://ex/q> <http://ex/B> .
"#;
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let n = |q: &str| match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => rows.len(), r => panic!("{r:?}") };
    assert_eq!(n("SELECT ?a ?b WHERE { VALUES ?b { <http://ex/B> } { ?l <http://ex/p> ?a . ?r <http://ex/q> ?b . FILTER(?a=?b) } }"), 0);
    assert_eq!(n("SELECT ?a ?b WHERE { VALUES ?b { <http://ex/A> } { ?l <http://ex/p> ?a . ?r <http://ex/q> ?b . FILTER(?a=?b) } }"), 1);
    assert_eq!(n("SELECT ?a ?b WHERE { ?l <http://ex/p> ?a . ?r <http://ex/q> ?b . FILTER(?a=?b) }"), 1);
}
