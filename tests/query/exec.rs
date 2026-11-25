use triplelight::exec::{query, QueryResult};
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
    let run = |wcoj| match triplelight::exec::query_with(&store(), q, &triplelight::exec::Options { stats: None, wcoj, raw_ids: false, general_group: false, count_pushdown: true, range_pushdown: true, char_pairs: true, sorted_probe: true, csr_paths: true, entailment: Default::default(), interrupt: None, memory_budget: None }).unwrap() {
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
    let opts = triplelight::exec::Options { raw_ids: true, ..Default::default() };
    let QueryResult::Ids(sol) = triplelight::exec::query_with(&s, q, &opts).unwrap() else { panic!() };
    let fast = triplelight::exec::results::serialize_ids(&sol, &s).unwrap();
    let slow = triplelight::exec::results::serialize(&query(&s, q).unwrap()).1;
    assert_eq!(fast, slow);
}

#[test]
fn errors_carry_their_kind() {
    use triplelight::exec::QueryErrorKind;
    assert_eq!(query(&store(), "SELECT WHERE {").unwrap_err().kind, QueryErrorKind::Syntax);
    assert_eq!(query(&store(), "SELECT * WHERE { ?s ?p ?o } LIMIT 1 FROM <urn:g>").unwrap_err().kind, QueryErrorKind::Syntax);
    assert_eq!(query(&store(), "SELECT * FROM <urn:g> WHERE { ?s ?p ?o }").unwrap_err().kind, QueryErrorKind::Unsupported);
}

/// A dictionary failure during query evaluation is a Storage error, not a missing result.
#[test]
fn dictionary_errors_are_storage_errors() {
    use triplelight::model::{Perm, TermId, TripleBatch};
    use triplelight::storage::{Dictionary, Snapshot, StorageError};
    struct BadDict(MemorySnapshot);
    impl Dictionary for BadDict {
        fn lookup(&self, _: &triplelight_rdf::Term) -> Result<Option<TermId>, StorageError> { Err(StorageError::other("planted dictionary failure")) }
        fn resolve(&self, id: TermId) -> Result<triplelight_rdf::Term, StorageError> { self.0.resolve(id) }
        fn with_term(&self, id: TermId, f: &mut dyn FnMut(triplelight_rdf::TermRef<'_>)) -> Result<(), StorageError> { self.0.with_term(id, f) }
    }
    impl Snapshot for BadDict {
        fn scan<'a>(&'a self, p: Perm, pre: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> { self.0.scan(p, pre) }
        fn count(&self, p: Perm, pre: &[TermId]) -> Result<u64, StorageError> { self.0.count(p, pre) }
        fn seek(&self, p: Perm, pre: &[TermId], t: TermId) -> Result<Option<TermId>, StorageError> { self.0.seek(p, pre, t) }
    }
    let e = query(&BadDict(store()), "SELECT ?x WHERE { ?x <http://ex/knows> <http://ex/bob> }").unwrap_err();
    assert_eq!(e.kind, triplelight::exec::QueryErrorKind::Storage);
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
    // Separate numeric predicates share decoded term values for the lifetime of the BGP.
    assert_eq!(
        run("SELECT ?x WHERE { ?x <http://ex/v> ?v FILTER(?v >= 5) FILTER(?v < 6) }"),
        run("SELECT ?x WHERE { ?x <http://ex/v> ?v FILTER(?v >= 5 && ?v < 6) }")
    );
    // A VALUES term absent from the store has a query-local scratch ID; sharing remains scoped to this BGP.
    assert_eq!(
        run("SELECT ?x WHERE { VALUES ?v { 42 } ?x <http://ex/v> ?stored FILTER(?v > 41) FILTER(?v < 43) }").len(),
        7
    );
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
    let run_with = |q: &str, general_group: bool| -> Vec<String> {
        let opts = triplelight::exec::Options { general_group, ..Default::default() };
        match triplelight::exec::query_with(&s, q, &opts).unwrap() { QueryResult::Solutions { rows, .. } => rows.into_iter().map(|r| format!("{r:?}")).collect(), r => panic!("{r:?}") }
    };
    let run = |q: &str| run_with(q, false);
    let queries = [
        "SELECT ?g (COUNT(*) AS ?n) (COUNT(?v) AS ?c) (SUM(?v) AS ?s) (AVG(?v) AS ?a) WHERE { ?x <http://ex/g> ?g OPTIONAL { ?x <http://ex/v> ?v } } GROUP BY ?g",
        "SELECT ?g (COUNT(*) AS ?n) WHERE { VALUES (?g ?v) { (<urn:b> UNDEF) (UNDEF 1) (<urn:a> 2) (<urn:b> 3) (UNDEF UNDEF) } } GROUP BY ?g",
        "SELECT ?g (COUNT(?v) AS ?n) WHERE { VALUES (?g ?v) { (<urn:b> UNDEF) (UNDEF 1) (<urn:a> 2) (<urn:b> 3) (UNDEF UNDEF) } } GROUP BY ?g",
        "SELECT ?g (MIN(?v) AS ?lo) (MAX(?v) AS ?hi) (SAMPLE(?v) AS ?sm) WHERE { ?x <http://ex/g> ?g OPTIONAL { ?x <http://ex/v> ?v } } GROUP BY ?g",
        "SELECT (COUNT(DISTINCT ?g) AS ?n) (SUM(DISTINCT ?v) AS ?s) (COUNT(DISTINCT *) AS ?d) WHERE { ?x <http://ex/g> ?g OPTIONAL { ?x <http://ex/v> ?v } }",
        "SELECT (COUNT(*) AS ?n) (SUM(?v) AS ?s) (AVG(?v) AS ?a) WHERE { ?x <http://ex/none> ?v }",
        "SELECT ?g (AVG(?v) AS ?a) WHERE { ?x <http://ex/g> ?g OPTIONAL { ?x <http://ex/v> ?v } } GROUP BY ?g HAVING (COUNT(?v) > 1)",
    ];
    let fast: Vec<_> = queries.iter().map(|q| run(q)).collect();
    let legacy: Vec<_> = queries.iter().map(|q| run_with(q, true)).collect();
    for ((q, f), l) in queries.iter().zip(&fast).zip(&legacy) { assert_eq!(f, l, "{q}"); }
}

/// Workers fold each group in member order, preserving promotion, overflow, and unbound results.
#[test]
fn parallel_expression_group_matches_serial_typed_rows() {
    let mut input = String::new();
    for _ in 0..2_048 {
        input.push_str(
            "(<urn:float> 1e16) (<urn:overflow> 9223372036854775807) \
             (<urn:float> 1e0) (<urn:overflow> 1) \
             (<urn:float> -1e16) (<urn:overflow> -1) \
             (<urn:error> UNDEF) (<urn:error> 4) ",
        );
    }
    let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build().unwrap();
    let snapshot = store();
    for aggregate in ["SUM(?x + 0)", "AVG(xsd:float(xsd:string(?x)))"] {
        let query = format!(
            "PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> \
             SELECT ?g ({aggregate} AS ?value) \
             WHERE {{ VALUES (?g ?x) {{ {input} }} }} GROUP BY ?g"
        );
        let run = |general_group| {
            let opts = triplelight::exec::Options { general_group, ..Default::default() };
            pool.install(|| match triplelight::exec::query_with(&snapshot, &query, &opts).unwrap() {
                QueryResult::Solutions { vars, rows } => (vars, rows),
                other => panic!("expected solution rows, got {other:?}"),
            })
        };
        let serial = run(true);
        assert_eq!(serial.1.len(), 3);
        assert_eq!(run(false), serial, "{aggregate}");
    }
}

#[test]
fn projected_order_expression_keeps_ties_and_unbound_results() {
    let snapshot = store();
    let run = |order| {
        let query_text = format!(
            "SELECT ?s (?n / ?d AS ?ratio) WHERE {{
               VALUES (?s ?n ?d) {{
                 (<urn:z> 6 3) (<urn:a> 4 2) (<urn:middle> 4 4)
                 (<urn:zero> 1 0) (<urn:missing> 1 UNDEF)
               }}
             }} ORDER BY DESC({order}) OFFSET 1 LIMIT 3"
        );
        match query(&snapshot, &query_text).unwrap() {
            QueryResult::Solutions { rows, .. } => rows,
            other => panic!("expected solution rows, got {other:?}"),
        }
    };
    let reused = run("?n / ?d");
    let evaluated = run("IF(true, ?n / ?d, 0)");
    assert_eq!(reused, evaluated);
    assert_eq!(reused.len(), 3);
    for (row, expected) in reused.iter().zip(["<urn:a>", "<urn:middle>", "<urn:zero>"]) {
        assert_eq!(row[0].as_ref().unwrap().to_string(), expected);
    }
    assert!(reused[2][1].is_none());
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
            triplelight_rdf::Term::Literal(l) => l.datatype().as_str().rsplit('#').next().unwrap().to_string(), t => panic!("{t}") }, r => panic!("{r:?}") }
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
    let rows = |q: &str, general_group: bool| -> Vec<Vec<String>> {
        let opts = triplelight::exec::Options { general_group, ..Default::default() };
        match triplelight::exec::query_with(&s, q, &opts).unwrap() { QueryResult::Solutions { rows, .. } => {
            let mut v: Vec<Vec<String>> = rows.into_iter().map(|r| r.into_iter().map(|t| t.map_or("-".into(), |t| t.to_string())).collect()).collect(); v.sort(); v }
            r => panic!("{r:?}") }
    };
    let int = |n: i64| format!("\"{n}\"^^<http://www.w3.org/2001/XMLSchema#integer>");
    let q1 = "SELECT ?g (SUM(?v) AS ?s) (COUNT(?v) AS ?c) (MIN(?v) AS ?mn) (MAX(?v) AS ?mx) (AVG(?v) AS ?a) WHERE { ?x <urn:g> ?g OPTIONAL { ?x <urn:v> ?v } } GROUP BY ?g";
    let q2 = "SELECT ?g (SUM(?v*2) AS ?s) (COUNT(?v*2) AS ?c) (MIN(?v*2) AS ?mn) (GROUP_CONCAT(?v*2) AS ?gc) WHERE { ?x <urn:g> ?g OPTIONAL { ?x <urn:v> ?v } } GROUP BY ?g";
    for legacy in [false, true] {
        assert_eq!(rows(q1, legacy), vec![
            vec!["\"x\"".into(), "-".into(), int(2), "-".into(), "-".into(), "-".into()],
            vec!["\"y\"".into(), "-".into(), int(1), "-".into(), "-".into(), "-".into()],
            vec!["\"z\"".into(), int(3), int(1), int(3), int(3), "\"3.0\"^^<http://www.w3.org/2001/XMLSchema#decimal>".into()],
        ], "legacy={legacy}");
        assert_eq!(rows(q2, legacy), vec![
            vec!["\"x\"".into(), "-".into(), int(1), "-".into(), "-".into()],
            vec!["\"y\"".into(), "-".into(), int(1), "-".into(), "-".into()],
            vec!["\"z\"".into(), int(6), int(1), int(6), "\"6\"".into()],
        ], "legacy={legacy}");
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

/// Subquery join (hash path when a column is bound on both sides) keeps the nested-loop join's answers, including
/// partially bound mappings from OPTIONAL.
#[test]
fn subquery_hash_join_matches_nested_loop() {
    let mut data = String::new();
    for i in 0..60 { data.push_str(&format!("<urn:p{i}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Product> .\n<urn:p{i}> <urn:f> <urn:f{}> .\n", i % 7)); if i % 3 == 0 { data.push_str(&format!("<urn:p{i}> <urn:o> \"x{i}\" .\n")) } }
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let rows = |q: &str| -> Vec<String> { match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => { let mut v: Vec<String> = rows.into_iter().map(|r| format!("{r:?}")).collect(); v.sort(); v } r => panic!("{r:?}") } };
    let q = "SELECT ?p ?n ?o WHERE { ?p a <urn:Product> OPTIONAL { ?p <urn:o> ?o } { SELECT ?p (COUNT(?f) AS ?n) WHERE { ?p <urn:f> ?f } GROUP BY ?p } }";
    let r = rows(q);
    assert_eq!(r.len(), 60);
    // Same answers as an equivalent query without the subquery.
    let flat = rows("SELECT ?p ?n ?o WHERE { ?p a <urn:Product> OPTIONAL { ?p <urn:o> ?o } ?p <urn:f> ?f BIND(1 AS ?n) }");
    assert_eq!(r.iter().map(|x| x.replace("\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>", "N")).collect::<Vec<_>>(), flat.iter().map(|x| x.replace("\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>", "N")).collect::<Vec<_>>());
}

#[test]
fn expression_uses_query_values_and_correlated_row() {
    let result = rows(r#"
        BASE <https://example.org/base/>
        SELECT ?x ?iri ?time ?hit WHERE {
            VALUES ?x { <http://ex/alice> <http://ex/bob> }
            BIND(IRI("item") AS ?iri)
            BIND(NOW() AS ?time)
            BIND(EXISTS { ?x <http://ex/knows> <http://ex/bob> } AS ?hit)
        }
    "#);
    assert_eq!(result.len(), 2);
    for row in &result {
        assert_eq!(row[1], "<https://example.org/base/item>");
        assert!(row[2].ends_with("^^<http://www.w3.org/2001/XMLSchema#dateTime>"));
    }
    assert_eq!(result[0][2], result[1][2]);
    assert_eq!(result[0][3], "\"true\"^^<http://www.w3.org/2001/XMLSchema#boolean>");
    assert_eq!(result[1][3], "\"false\"^^<http://www.w3.org/2001/XMLSchema#boolean>");
}

/// NOT EXISTS / EXISTS results are memoized per binding of the pattern's variables; rows that share those
/// bindings but differ elsewhere must still get the right answer.
#[test]
fn exists_memo_matches_per_row_semantics() {
    let mut data = String::new();
    let ty = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    for i in 0..30 {
        data.push_str(&format!("<urn:p{i}> {ty} <urn:T> .\n"));
        if i % 4 == 0 { data.push_str(&format!("<urn:p{i}> <urn:feat> <urn:f{}> .\n", i % 3)) }
        for j in 0..5 { data.push_str(&format!("<urn:o{i}_{j}> <urn:product> <urn:p{i}> .\n<urn:o{i}_{j}> <urn:price> \"{}\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n", i * 10 + j)) }
    }
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let n = |q: &str| match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => rows.len(), r => panic!("{r:?}") };
    let feats = "VALUES ?f { <urn:f0> <urn:f1> <urn:f2> }";
    // 30 products x 5 offers x 3 features = 450 rows; products 0,4,..,28 (8 products) each have exactly one feature.
    assert_eq!(n(&format!("SELECT * {{ {feats} ?p a <urn:T> . ?o <urn:product> ?p ; <urn:price> ?x . FILTER NOT EXISTS {{ ?p <urn:feat> ?f }} }}")), 450 - 8 * 5);
    assert_eq!(n(&format!("SELECT * {{ {feats} ?p a <urn:T> . ?o <urn:product> ?p ; <urn:price> ?x . FILTER EXISTS {{ ?p <urn:feat> ?f }} }}")), 8 * 5);
    // An EXISTS filter that depends on a non-key row variable through its own FILTER.
    assert_eq!(n("SELECT * { ?o <urn:product> ?p ; <urn:price> ?x . FILTER EXISTS { ?p a <urn:T> FILTER(?x > 100) } }"), 99);
}

/// Aggregate arguments over one variable are memoized per term ID across groups; every group must still see
/// its own values (and an error value must still poison the aggregate).
#[test]
fn aggregate_argument_memo_is_per_value() {
    let xi = "<http://www.w3.org/2001/XMLSchema#integer>";
    let data = format!("<urn:a> <urn:v> \"1\"^^{xi} .\n<urn:a> <urn:v> \"2\"^^{xi} .\n<urn:b> <urn:v> \"2\"^^{xi} .\n<urn:b> <urn:v> \"3\"^^{xi} .\n<urn:c> <urn:v> \"x\" .\n<urn:c> <urn:v> \"2\"^^{xi} .\n");
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let q = "SELECT ?g (SUM(?x * 10) AS ?s) (GROUP_CONCAT(STR(?x); separator=\",\") AS ?c) WHERE { ?g <urn:v> ?x } GROUP BY ?g ORDER BY ?g";
    let rows = match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => rows, r => panic!("{r:?}") };
    let txt: Vec<String> = rows.iter().map(|r| format!("{r:?}")).collect();
    assert_eq!(rows.len(), 3);
    assert!(txt[0].contains("\"30\"") && txt[1].contains("\"50\""), "{txt:?}");
    // ?x * 10 is an error for "x": SUM is unbound for <urn:c>, GROUP_CONCAT still has both values.
    assert!(!txt[2].contains("\"20\"") && (txt[2].contains("x,2") || txt[2].contains("2,x")), "{txt:?}");
}

/// Regression (found by Jianlin): the equality rewrite evaluates temporary cloned filters, so pattern addresses
/// are reused within one query. The EXISTS memo must not return a result cached for a different pattern.
#[test]
fn exists_memo_survives_reused_temporary_patterns() {
    let e = "http://example.org/long/";
    let data = format!("<{e}s> <{e}p> <{e}A> .\n<{e}t> <{e}q> <{e}A> .\n<{e}A> <{e}yes> <{e}Z> .\n");
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let branch = |p: &str| format!("{{ ?s <{e}p> ?a . ?t <{e}q> ?b . FILTER(?a=?b && EXISTS {{ ?a <{e}{p}> ?z }}) }}");
    let q = format!("SELECT ?a ?b WHERE {{ {} }}", (0..12).map(|i| branch(if i % 2 == 0 { "yes" } else { "no" })).collect::<Vec<_>>().join(" UNION "));
    match query(&s, &q).unwrap() { QueryResult::Solutions { rows, .. } => assert_eq!(rows.len(), 6), r => panic!("{r:?}") }
}

/// Single-variable filters are memoized per term ID inside a BGP; results must equal per-row evaluation,
/// including for non-numeric (date) comparisons and unbound/erroring values.
#[test]
fn single_variable_filter_memo_matches_per_row() {
    let xd = "<http://www.w3.org/2001/XMLSchema#date>";
    let mut data = String::new();
    for i in 0..200 {
        let d = format!("2007-10-{:02}", 1 + i % 28);
        data.push_str(&format!("<urn:r{i}> <urn:for> <urn:p{}> .\n<urn:r{i}> <urn:date> \"{d}\"^^{xd} .\n", i % 9));
    }
    data.push_str("<urn:rx> <urn:for> <urn:p0> .\n<urn:rx> <urn:date> \"not a date\" .\n");
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let n = |q: &str| match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => rows.len(), r => panic!("{r:?}") };
    // Days 10..=20 inclusive of 1..=28: i % 28 in 9..=19 -> 11 of every 28 values.
    let expect = (0..200).filter(|i| (9..=19).contains(&(i % 28))).count();
    assert_eq!(n(&format!("SELECT * {{ ?r <urn:for> ?p ; <urn:date> ?d FILTER(?d >= \"2007-10-10\"^^{xd} && ?d <= \"2007-10-20\"^^{xd}) }}")), expect);
    assert_eq!(n(&format!("SELECT * {{ ?r <urn:for> ?p ; <urn:date> ?d FILTER(!(?d >= \"2007-10-10\"^^{xd})) }}")), (0..200).filter(|i| i % 28 < 9).count());
}

/// Fully bound probes `?x <p> <o>` over many rows use a scanned membership set; answers and multiplicities
/// must match the per-row probe join.
#[test]
fn member_set_probe_matches_probe_join() {
    let mut data = String::new();
    for i in 0..3000 {
        data.push_str(&format!("<urn:r{i}> <urn:by> <urn:u{}> .\n", i % 1500));
    }
    for u in 0..1500 { if u % 3 == 0 { data.push_str(&format!("<urn:u{u}> <urn:country> <urn:US> .\n")) } else { data.push_str(&format!("<urn:u{u}> <urn:country> <urn:AT> .\n")) } }
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let n = |q: &str| match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => rows.len(), r => panic!("{r:?}") };
    assert_eq!(n("SELECT * { ?r <urn:by> ?u . ?u <urn:country> <urn:US> }"), 1000);
    assert_eq!(n("SELECT (COUNT(*) AS ?c) { ?r <urn:by> ?u . ?u <urn:country> <urn:US> }"), 1);
    assert_eq!(n("SELECT * { ?r <urn:by> ?u . ?u <urn:country> <urn:XX> }"), 0);
}

/// `FILTER [NOT] EXISTS` over a non-BGP input (here a subquery) is evaluated as one batched semi/anti-join; it must
/// keep per-row semantics, including an inner FILTER that reads an outer variable.
#[test]
fn exists_batch_matches_per_row_semantics() {
    let mut data = String::new();
    for i in 0..40 {
        if i % 4 == 0 { data.push_str(&format!("<urn:p{i}> <urn:feat> <urn:f> .\n")) }
        for j in 0..5 { data.push_str(&format!("<urn:o{i}_{j}> <urn:product> <urn:p{i}> .\n<urn:o{i}_{j}> <urn:price> \"{}\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n", i * 10 + j)) }
    }
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let n = |q: &str| match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => rows.len(), r => panic!("{r:?}") };
    let sub = "{ SELECT ?o ?p ?x { ?o <urn:product> ?p ; <urn:price> ?x } }";
    // 200 offers; products 0,4,..,36 (10 products x 5 offers) have the feature.
    assert_eq!(n(&format!("SELECT * {{ {sub} FILTER NOT EXISTS {{ ?p <urn:feat> ?f }} }}")), 150);
    assert_eq!(n(&format!("SELECT * {{ {sub} FILTER EXISTS {{ ?p <urn:feat> ?f }} }}")), 50);
    // Inner filter on an outer variable: featured products with price > 200 -> products 24,28,32,36 (20 offers;
    // product 20 has prices 200..204 of which 4 are > 200).
    assert_eq!(n(&format!("SELECT * {{ {sub} FILTER EXISTS {{ ?p <urn:feat> ?f FILTER(?x > 200) }} }}")), 24);
}

/// Hash join for `(const, bound, free)` patterns over many rows must give the same rows, multiplicities and
/// per-row value order as per-row probes, including keys with zero or several matches.
#[test]
fn hash_extend_matches_probe_join() {
    let mut data = String::new();
    let mut expect = 0usize;
    for i in 0..6000 {
        data.push_str(&format!("<urn:s{i}> <urn:a> <urn:x{}> .\n", i % 13));
        let k = i % 4;
        for j in 0..k { data.push_str(&format!("<urn:s{i}> <urn:b> \"v{j}\" .\n")) }
        expect += k;
    }
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let rows = |q: &str| -> Vec<String> { match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => rows.into_iter().map(|r| format!("{r:?}")).collect(), r => panic!("{r:?}") } };
    let joined = rows("SELECT ?s ?x ?y WHERE { ?s <urn:a> ?x . ?s <urn:b> ?y }");
    assert_eq!(joined.len(), expect);
    let few = rows("SELECT ?s ?x ?y WHERE { VALUES ?s { <urn:s7> <urn:s4> <urn:s3> } ?s <urn:a> ?x . ?s <urn:b> ?y }");
    assert_eq!(few.len(), 6);
    for r in &few { assert!(joined.contains(r), "{r}") }
    // Reverse direction: object bound, subject free.
    assert_eq!(rows("SELECT ?x ?s WHERE { ?s <urn:a> ?x . ?t <urn:a> ?x }").len(), (0..13).map(|m| { let c = (0..6000).filter(|i| i % 13 == m).count(); c * c }).sum::<usize>());
}

/// Features the evaluator does not implement must fail with `Unsupported`, never return an answer: a GRAPH pattern
/// must not be answered from the default graph, and NOW()/RAND()/hashes/unknown functions must not silently make
/// every FILTER false. Supported XSD casts and functions keep working.
#[test]
fn unsupported_features_are_rejected_not_misanswered() {
    use triplelight::exec::QueryErrorKind;
    let s = MemorySnapshot::from_ntriples(&b"<urn:a> <urn:p> \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n"[..]).unwrap();
    let rejected = [
        "SELECT * WHERE { GRAPH <urn:g> { ?s ?p ?o } }",
        "SELECT * WHERE { GRAPH ?g { ?s ?p ?o } }",
        "SELECT * FROM <urn:g> WHERE { ?s ?p ?o }",
        "SELECT * FROM NAMED <urn:g> WHERE { ?s ?p ?o }",
        "SELECT * WHERE { ?s ?p ?o FILTER(<urn:my:fn>(?o)) }",
        "SELECT * WHERE { ?s ?p ?o FILTER EXISTS { GRAPH ?g { ?s ?p ?o } } }",
    ];
    for q in rejected {
        match query(&s, q) {
            Err(e) => assert_eq!(e.kind, QueryErrorKind::Unsupported, "{q}: {e:?}"),
            Ok(r) => panic!("{q} should be rejected, got {r:?}"),
        }
    }
    let accepted = [
        "SELECT (BNODE(\"x\") AS ?b) WHERE { ?s ?p ?o }",
        "SELECT (xsd:double(?o) AS ?d) (STR(?o) AS ?str) WHERE { ?s ?p ?o }",
        "SELECT * WHERE { ?s ?p ?o FILTER(xsd:integer(?o) = 1 && REGEX(STR(?o), \"1\")) }",
        "SELECT * WHERE { ?s ?p ?o FILTER(?o < NOW()) }",
        "SELECT (RAND() AS ?r) (UUID() AS ?u) (STRUUID() AS ?v) (BNODE() AS ?b) WHERE { ?s ?p ?o }",
        "SELECT (MD5(STR(?o)) AS ?h) (SHA1(STR(?o)) AS ?h1) (SHA512(STR(?o)) AS ?h5) WHERE { ?s ?p ?o }",
        "SELECT * WHERE { ?s <urn:p>+ ?o }",
    ];
    for q in accepted {
        let q = format!("PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> {q}");
        assert!(query(&s, &q).is_ok(), "{q}");
    }
}

/// SPARQL evaluates groups bottom-up: a FILTER or BIND inside a group must not see variables bound only outside it
/// (W3C bind07, bind10). Sideways passing is kept only where it cannot change the answer.
#[test]
fn group_scoping_is_bottom_up() {
    let xi = "<http://www.w3.org/2001/XMLSchema#integer>";
    let data = format!("<urn:s1> <urn:p> \"1\"^^{xi} .\n<urn:s4> <urn:p> \"4\"^^{xi} .\n");
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let rows = |q: &str| -> Vec<String> { match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => { let mut v: Vec<String> = rows.into_iter().map(|r| format!("{r:?}")).collect(); v.sort(); v } r => panic!("{r:?}") } };
    // bind07: ?o is not in scope inside the UNION branches, so ?z is unbound in all 4 rows.
    let r = rows("SELECT ?s ?o ?z { ?s ?p ?o . { BIND(?o+1 AS ?z) } UNION { BIND(?o+2 AS ?z) } }");
    assert_eq!(r.len(), 4);
    assert!(r.iter().all(|x| !x.contains("\"2\"") && !x.contains("\"3\"") && !x.contains("\"5\"") && !x.contains("\"6\"")), "{r:?}");
    // bind10: ?z is not in scope in the inner FILTER, so it filters everything out.
    assert!(rows("SELECT ?s ?v ?z { BIND(4 AS ?z) { ?s <urn:p> ?v . FILTER(?v = ?z) } }").is_empty());
    // The same filter at the outer level does see ?z.
    assert_eq!(rows("SELECT ?s ?v ?z { BIND(4 AS ?z) ?s <urn:p> ?v . FILTER(?v = ?z) }").len(), 1);
    // OPTIONAL { P FILTER(F) } is LeftJoin(L, P, F): F sees the merged solution, so ?v > 2 matches s4 only.
    let c = rows("SELECT (COUNT(?x) AS ?c) { ?s <urn:p> ?v OPTIONAL { ?s <urn:p> ?x FILTER(?v > 2) } }");
    assert!(c[0].contains("\"1\""), "{c:?}");
    // A FILTER one group deeper is not the LeftJoin condition: there ?v is out of scope, so nothing matches.
    let c = rows("SELECT (COUNT(?x) AS ?c) { ?s <urn:p> ?v OPTIONAL { { ?s <urn:p> ?x FILTER(?v > 2) } } }");
    assert!(c[0].contains("\"0\""), "{c:?}");
}

/// A VALUES variable whose value is UNDEF is not bound: an outer binding of the same name must not leak into an inner
/// BIND (Jianlin's counterexample to the first scoping fix; agrees with Jena).
#[test]
fn values_undef_does_not_leak_outer_binding() {
    let s = MemorySnapshot::from_ntriples(&b""[..]).unwrap();
    let rows = |q: &str| -> Vec<String> { match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => rows.into_iter().map(|r| format!("{r:?}")).collect(), r => panic!("{r:?}") } };
    let r = rows("SELECT ?x ?y WHERE { VALUES ?x { 1 } { VALUES ?x { UNDEF } BIND(?x AS ?y) } }");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].matches("Some(").count(), 1, "y must be unbound: {r:?}");
    // Control: an inner bound value is visible to BIND.
    let r = rows("SELECT ?x ?y WHERE { VALUES ?x { 1 } { VALUES ?x { 1 } BIND(?x AS ?y) } }");
    assert_eq!(r[0].matches("Some(").count(), 2, "{r:?}");
}

/// An inner group's BIND produces its own solution, which is then joined with the outer one: `{ BIND(2 AS ?x) }`
/// is incompatible with an outer x=1 (no rows), compatible with x=2, and an erroring BIND leaves the outer value.
#[test]
fn inner_bind_joins_with_outer_binding() {
    let s = MemorySnapshot::from_ntriples(&b""[..]).unwrap();
    let n = |q: &str| match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => rows.len(), r => panic!("{r:?}") };
    assert_eq!(n("SELECT ?x WHERE { VALUES ?x {1} { BIND(2 AS ?x) } }"), 0);
    assert_eq!(n("SELECT ?x WHERE { VALUES ?x {2} { BIND(2 AS ?x) } }"), 1);
    assert_eq!(n("SELECT ?x WHERE { VALUES ?x {1} { BIND(1/0 AS ?x) } }"), 1);
}

/// xsd:decimal arithmetic is exact (W3C agg-sum-01): sums, comparisons and division of decimals do not go through
/// binary floating point. Float/double keep IEEE semantics.
#[test]
fn decimal_arithmetic_is_exact() {
    let xd = "<http://www.w3.org/2001/XMLSchema#decimal>";
    let data = format!("<urn:a> <urn:v> \"1.1\"^^{xd} .\n<urn:b> <urn:v> \"10.0\"^^{xd} .\n<urn:c> <urn:w> \"0.1\"^^{xd} .\n<urn:c> <urn:x> \"0.2\"^^{xd} .\n");
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let one = |q: &str| -> String { match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => format!("{:?}", rows), r => panic!("{r:?}") } };
    assert!(one("SELECT (SUM(?v) AS ?s) { ?x <urn:v> ?v }").contains("\"11.1\""), "{}", one("SELECT (SUM(?v) AS ?s) { ?x <urn:v> ?v }"));
    // 0.1 + 0.2 = 0.3 exactly for decimals (false in binary floating point).
    assert!(one("SELECT ?c { ?c <urn:w> ?w ; <urn:x> ?y FILTER(?w + ?y = 0.3) }").contains("urn:c"));
    assert!(one("SELECT ?c { ?c <urn:w> ?w ; <urn:x> ?y FILTER(?w + ?y > 0.3) }") == "[]");
    // Integer division is decimal, AVG is exact.
    assert!(one("SELECT ((1 / 4) AS ?q) {}").contains("\"0.25\""));
    assert!(one("SELECT (AVG(?v) AS ?a) { ?x <urn:v> ?v }").contains("\"5.55\""));
    // Division by zero is an error for decimals; doubles give INF.
    assert!(!one("SELECT ((1.0 / 0) AS ?q) {}").contains("Some"));
    assert!(one("SELECT ((1.0e0 / 0) AS ?q) {}").contains("INF"));
}

/// Jianlin's decimal controls: ROUND at the Decimal maximum does not panic; a decimal beyond the representable
/// precision is an error, never a silently different value; float promotion converts each operand to binary32 first.
#[test]
fn decimal_edge_cases_and_float_promotion() {
    let s = MemorySnapshot::from_ntriples(&b""[..]).unwrap();
    let one = |q: &str| -> String { match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => format!("{rows:?}"), r => panic!("{r:?}") } };
    assert!(one("SELECT (ROUND(79228162514264337593543950335.0) AS ?r) {}").contains("79228162514264337593543950335"));
    // 1e-29 is beyond Decimal's 28 fractional digits: an error (unbound), not 0.0.
    let r = one("SELECT ((0.00000000000000000000000000001 + 0.0) AS ?r) {}");
    assert!(!r.contains("Some"), "{r}");
    let x = "PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> ";
    // decimal 16777217 promoted to float is 16777216.
    assert!(one(&format!("{x} SELECT ((16777217.0 + xsd:float(\"-16777216\")) AS ?r) {{}}")).contains("\"0.0\"") || one(&format!("{x} SELECT ((16777217.0 + xsd:float(\"-16777216\")) AS ?r) {{}}")).contains("\"0\""));
    assert!(one(&format!("{x} SELECT ?ok {{ BIND(16777217.0 = xsd:float(\"16777216\") AS ?ok) }}")).contains("true"));
}

/// Jianlin's boundary controls: results beyond exact Decimal range are errors, in either operand order; exact results
/// inside the range are exact (including terminating quotients).
#[test]
fn decimal_boundaries_are_exact_or_error() {
    let s = MemorySnapshot::from_ntriples(&b""[..]).unwrap();
    let v = |q: &str| -> String { match query(&s, q).unwrap() { QueryResult::Solutions { rows, .. } => format!("{rows:?}"), r => panic!("{r:?}") } };
    let max = "79228162514264337593543950335";
    for q in [format!("SELECT (({max} + 0.1) AS ?r) {{}}"), format!("SELECT ((0.1 + {max}) AS ?r) {{}}"),
              "SELECT ((99999999999999.9 * 99999999999999.9) AS ?r) {}".to_string(),
              "SELECT ((0.0000000000000000000000000001 / 10.0) AS ?r) {}".to_string()] {
        assert!(!v(&q).contains("Some"), "{q} must be an error: {}", v(&q));
    }
    assert!(v("SELECT ((9999999.9 * 9999999.9) AS ?r) {}").contains("99999998000000.01"));
    assert!(v("SELECT ((1.0 / 8) AS ?r) {}").contains("0.125"));
    assert!(v("SELECT ((1.0 / 3) AS ?r) {}").contains("0.333333"));
}

#[test]
fn describe_returns_subject_triples_and_blank_node_closure() {
    let data = "<urn:a> <urn:p> <urn:b> .\n<urn:a> <urn:q> _:x .\n_:x <urn:r> \"deep\" .\n<urn:b> <urn:p> <urn:c> .\n<urn:z> <urn:p> <urn:a> .\n";
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let count = |q: &str| match query(&s, q).unwrap() { QueryResult::Graph(t) => t.len(), r => panic!("{r:?}") };
    // <urn:a>'s own two triples plus the blank node's triple; <urn:z>'s triple about <urn:a> is not included.
    assert_eq!(count("DESCRIBE <urn:a>"), 3);
    assert_eq!(count("DESCRIBE ?x WHERE { ?x <urn:p> <urn:c> }"), 1);
    assert_eq!(count("DESCRIBE <urn:a> <urn:b>"), 4);
    assert_eq!(count("DESCRIBE <urn:nothing>"), 0);
}

#[test]
fn describe_handles_blank_node_cycles_shared_nodes_and_projection() {
    let count = |data: &str, q: &str| {
        let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
        match query(&s, q).unwrap() { QueryResult::Graph(t) => t.len(), r => panic!("{r:?}") }
    };
    // A blank-node cycle terminates: a's triple plus the two cycle triples.
    assert_eq!(count("<urn:a> <urn:q> _:x .\n_:x <urn:p> _:y .\n_:y <urn:p> _:x .\n", "DESCRIBE <urn:a>"), 3);
    // A blank node shared by two described resources is described once.
    assert_eq!(count("<urn:a> <urn:q> _:s .\n<urn:b> <urn:q> _:s .\n_:s <urn:r> \"v\" .\n", "DESCRIBE <urn:a> <urn:b>"), 3);
    // Only projected variables are described: ?y's resource <urn:b> is not.
    let d = "<urn:a> <urn:p> <urn:b> .\n<urn:b> <urn:q> \"1\" .\n";
    assert_eq!(count(d, "DESCRIBE ?x WHERE { ?x <urn:p> ?y }"), 1);
    assert_eq!(count(d, "DESCRIBE * WHERE { ?x <urn:p> ?y }"), 2);
}

/// A storage failure while describing is a Storage error, not a shorter description.
#[test]
fn describe_scan_errors_are_storage_errors() {
    use triplelight::model::{Perm, TermId, TripleBatch};
    use triplelight::storage::{Dictionary, Snapshot, StorageError};
    struct BadScan(MemorySnapshot);
    impl Dictionary for BadScan {
        fn lookup(&self, t: &triplelight_rdf::Term) -> Result<Option<TermId>, StorageError> { self.0.lookup(t) }
        fn resolve(&self, id: TermId) -> Result<triplelight_rdf::Term, StorageError> { self.0.resolve(id) }
        fn with_term(&self, id: TermId, f: &mut dyn FnMut(triplelight_rdf::TermRef<'_>)) -> Result<(), StorageError> { self.0.with_term(id, f) }
    }
    impl Snapshot for BadScan {
        fn scan<'a>(&'a self, _: Perm, _: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
            Box::new(std::iter::once(Err(StorageError::other("planted scan failure"))))
        }
        fn count(&self, p: Perm, pre: &[TermId]) -> Result<u64, StorageError> { self.0.count(p, pre) }
        fn seek(&self, p: Perm, pre: &[TermId], t: TermId) -> Result<Option<TermId>, StorageError> { self.0.seek(p, pre, t) }
    }
    let e = query(&BadScan(store()), "DESCRIBE <http://ex/alice>").unwrap_err();
    assert_eq!(e.kind, triplelight::exec::QueryErrorKind::Storage);
}

/// Deeply nested queries fail with a syntax error instead of overflowing the stack (run in a thread with a small
/// stack so an unbounded recursion would crash the test).
#[test]
fn deep_nesting_is_a_syntax_error_not_a_stack_overflow() {
    let deep = |n: usize, open: &str, close: &str, inner: &str| format!("SELECT * WHERE {}{}{}", open.repeat(n), inner, close.repeat(n));
    let h = std::thread::Builder::new().stack_size(8 << 20).spawn(move || {
        let s = MemorySnapshot::from_ntriples(&b"<urn:a> <urn:p> \"1\" .\n"[..]).unwrap();
        let kind = |q: &str| query(&s, q).err().map(|e| e.kind);
        let groups = deep(100_000, "{ ", " }", "?s ?p ?o");
        let exprs = format!("SELECT * WHERE {{ ?s ?p ?o FILTER({}?o{}) }}", "(".repeat(100_000), ")".repeat(100_000));
        let lists = format!("SELECT * WHERE {{ ?s ?p {}1{} }}", "(".repeat(100_000), ")".repeat(100_000));
        let paths = format!("SELECT * WHERE {{ ?s {}<urn:p>{} ?o }}", "(".repeat(100_000), ")".repeat(100_000));
        // Flat syntax that builds deep algebra: long UNION / + / OPTIONAL chains.
        let unions = format!("SELECT * WHERE {{ {} }}", vec!["{ ?s ?p ?o }"; 100_000].join(" UNION "));
        let sums = format!("SELECT * WHERE {{ ?s ?p ?o FILTER({} > 0) }}", vec!["?o"; 100_000].join(" + "));
        let optionals = format!("SELECT * WHERE {{ ?s ?p ?o {} }}", "OPTIONAL { ?s ?p ?o } ".repeat(100_000));
        let flat_ok = format!("SELECT * WHERE {{ {} }}", vec!["{ ?s ?p ?o }"; 200].join(" UNION "));
        (kind(&groups), kind(&exprs), kind(&lists), kind(&paths), kind(&unions), kind(&sums), kind(&optionals),
         kind(&deep(50, "{ ", " }", "?s ?p ?o")), kind(&flat_ok))
    }).unwrap();
    let syntax = Some(triplelight::exec::QueryErrorKind::Syntax);
    let (g, e, l, p, u, s, o, ok, flat) = h.join().expect("parser and evaluator must not overflow the stack");
    assert_eq!([g, e, l, p, u, s, o], [syntax; 7]);
    assert_eq!(ok, None, "50 nested groups are well within the limit");
    assert_eq!(flat, None, "a 200-branch UNION is well within the limit and evaluates");
}

/// User variables may start with `_` (`?_person`); they are ordinary variables, distinct from internal ones and from
/// blank nodes with a similar label.
#[test]
fn underscore_variables_are_ordinary_variables() {
    let s = MemorySnapshot::from_ntriples(&b"<urn:a> <urn:p> <urn:b> .\n"[..]).unwrap();
    let solutions = |q: &str| match query(&s, q).unwrap() {
        QueryResult::Solutions { vars, rows } => (vars.iter().map(|v| v.as_str().to_string()).collect::<Vec<_>>(), rows.len()),
        r => panic!("{r:?}"),
    };
    assert_eq!(solutions("SELECT * WHERE { ?_person <urn:p> ?o }"), (vec!["_person".into(), "o".into()], 1));
    assert_eq!(solutions("SELECT * WHERE { { SELECT * WHERE { ?_s <urn:p> ?_o } } }"), (vec!["_s".into(), "_o".into()], 1));
    assert_eq!(solutions("SELECT * WHERE { ?s <urn:p> ?o BIND(?o AS ?_x) }"), (vec!["s".into(), "o".into(), "_x".into()], 1));
    // `?_bx` and the blank node `_:x` are different: the blank node matches anything, the variable is projected.
    assert_eq!(solutions("SELECT * WHERE { _:x <urn:p> ?_bx }"), (vec!["_bx".into()], 1));
}

/// A long flat property path (`p|p|…`) is bounded by the algebra depth check, not left to overflow the evaluator.
#[test]
fn long_flat_paths_are_bounded() {
    let h = std::thread::Builder::new().stack_size(8 << 20).spawn(|| {
        let s = MemorySnapshot::from_ntriples(&b"<urn:a> <urn:p> <urn:b> .\n"[..]).unwrap();
        let alt = |n: usize| format!("SELECT * WHERE {{ ?s ({})+ ?o }}", vec!["<urn:p>"; n].join("|"));
        (query(&s, &alt(5_000)).err().map(|e| e.kind), query(&s, &alt(100)).is_ok())
    }).unwrap();
    let (long, short_ok) = h.join().expect("must not overflow the stack");
    assert_eq!(long, Some(triplelight::exec::QueryErrorKind::Syntax));
    assert!(short_ok);
}

#[test]
fn construct_rows_scope_blank_nodes_and_skip_unbound_slots() {
    // Template blank nodes are fresh per solution; a template triple with an unbound variable is skipped.
    let q = "CONSTRUCT { ?x <http://ex/tag> _:t . ?x <http://ex/n> ?name } WHERE { ?x <http://ex/age> ?a OPTIONAL { ?x <http://ex/name> ?name } }";
    let QueryResult::Graph(triples) = query(&store(), q).unwrap() else { panic!() };
    let tags: Vec<_> = triples.iter().filter(|t| t.predicate.as_str() == "http://ex/tag").collect();
    assert_eq!(tags.len(), 3, "one tag per solution (alice, bob, carol)");
    let blanks: std::collections::HashSet<String> = tags.iter().map(|t| t.object.to_string()).collect();
    assert_eq!(blanks.len(), 3, "a fresh blank node per solution");
    // carol has no name: her ?name triple is skipped, alice's and bob's are kept.
    assert_eq!(triples.iter().filter(|t| t.predicate.as_str() == "http://ex/n").count(), 2);
    // Within one solution, repeated references to the same template blank node denote one node.
    let q = "CONSTRUCT { ?x <http://ex/tag> _:t . _:t <http://ex/of> ?x } WHERE { ?x <http://ex/age> ?a }";
    let QueryResult::Graph(triples) = query(&store(), q).unwrap() else { panic!() };
    for t in triples.iter().filter(|t| t.predicate.as_str() == "http://ex/tag") {
        let back: Vec<_> = triples.iter().filter(|u| u.predicate.as_str() == "http://ex/of" && u.subject.to_string() == t.object.to_string()).collect();
        assert_eq!(back.len(), 1, "same _:t within a solution");
        assert_eq!(back[0].object.to_string(), t.subject.to_string());
    }
}

#[test]
fn construct_template_blank_nodes_never_reuse_an_input_label() {
    // The input already has a blank node labelled exactly like the first generated one (`_:t` + `r` + row 0). The
    // generated node must not merge with it: the output edge links two different nodes, never a self-loop.
    let data = "_:tr0 <http://ex/q> <http://ex/v> .\n_:tr0_1 <http://ex/q> <http://ex/w> .\n";
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let q = "CONSTRUCT { _:t <http://ex/p> ?x } WHERE { ?x <http://ex/q> ?y } ORDER BY ?y";
    let QueryResult::Graph(triples) = query(&s, q).unwrap() else { panic!() };
    assert_eq!(triples.len(), 2);
    let inputs: std::collections::HashSet<String> = triples.iter().map(|t| t.object.to_string()).collect();
    assert_eq!(inputs, ["_:tr0", "_:tr0_1"].iter().map(|x| x.to_string()).collect(), "objects are the input nodes");
    let generated: std::collections::HashSet<String> = triples.iter().map(|t| t.subject.to_string()).collect();
    assert_eq!(generated.len(), 2, "a distinct fresh node per solution");
    assert!(generated.is_disjoint(&inputs), "generated labels avoid every bound input label: {generated:?}");
    for t in &triples { assert_ne!(t.subject.to_string(), t.object.to_string(), "no self-loop") }
}

#[test]
fn construct_template_labels_unchanged_without_collision() {
    // No bound input blank node collides, so the existing `<label>r<row>` labels are kept as they were.
    let q = "CONSTRUCT { ?x <http://ex/tag> _:t } WHERE { ?x <http://ex/age> ?a } ORDER BY ?x";
    let QueryResult::Graph(triples) = query(&store(), q).unwrap() else { panic!() };
    let mut labels: Vec<String> = triples.iter().map(|t| t.object.to_string()).collect();
    labels.sort();
    assert_eq!(labels, vec!["_:tr0", "_:tr1", "_:tr2"]);
}

/// The BGP loop's compact-tail fast path (`matching_tail`, key position `k` at `t[k - 1]`) gives the same answers as
/// the `scan` path for one and two bound key columns over every permutation, including repeated variables.
#[test]
fn matching_tail_path_matches_scan_path() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use triplelight::model::{Perm, TermId, TripleBatch};
    use triplelight::storage::{Dictionary, Snapshot, StorageError};
    struct View(MemorySnapshot, bool, AtomicUsize);
    impl Dictionary for View {
        fn lookup(&self, t: &triplelight_rdf::Term) -> Result<Option<TermId>, StorageError> { self.0.lookup(t) }
        fn resolve(&self, id: TermId) -> Result<triplelight_rdf::Term, StorageError> { self.0.resolve(id) }
        fn with_term(&self, id: TermId, f: &mut dyn FnMut(triplelight_rdf::TermRef<'_>)) -> Result<(), StorageError> { self.0.with_term(id, f) }
    }
    impl Snapshot for View {
        fn scan<'a>(&'a self, p: Perm, pre: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> { self.0.scan(p, pre) }
        fn count(&self, p: Perm, pre: &[TermId]) -> Result<u64, StorageError> { self.0.count(p, pre) }
        fn seek(&self, p: Perm, pre: &[TermId], t: TermId) -> Result<Option<TermId>, StorageError> { self.0.seek(p, pre, t) }
        fn matching_tail(&self, p: Perm, pre: &[TermId]) -> Option<&[[TermId; 2]]> {
            if !self.1 { return None }
            let r = self.0.matching_tail(p, pre);
            if r.is_some() { self.2.fetch_add(1, Ordering::Relaxed); }
            r
        }
    }
    let data = "<urn:a> <urn:p> <urn:b> .\n<urn:a> <urn:p> <urn:a> .\n<urn:b> <urn:p> <urn:c> .\n<urn:b> <urn:q> <urn:b> .\n\
                <urn:c> <urn:q> <urn:a> .\n<urn:c> <urn:p> <urn:a> .\n<urn:a> <urn:q> \"1\" .\n<urn:b> <urn:a> <urn:a> .\n";
    let sorted = |s: &View, q: &str| match query(s, q).unwrap() {
        QueryResult::Solutions { rows, .. } => {
            let mut v: Vec<String> = rows.into_iter().map(|r| format!("{r:?}")).collect();
            v.sort();
            v
        }
        r => panic!("{r:?}"),
    };
    let tail = View(MemorySnapshot::from_ntriples(data.as_bytes()).unwrap(), true, AtomicUsize::new(0));
    let scan = View(MemorySnapshot::from_ntriples(data.as_bytes()).unwrap(), false, AtomicUsize::new(0));
    let queries = [
        "SELECT * WHERE { ?x <urn:p> ?y . ?y <urn:p> ?z }",
        "SELECT * WHERE { ?x <urn:p> ?y . ?y ?r ?z }",
        "SELECT * WHERE { ?x <urn:p> ?y . ?z ?r ?y }",
        "SELECT * WHERE { ?x <urn:p> ?y . ?z ?y ?w }",
        "SELECT * WHERE { ?x <urn:p> ?y . ?y ?r ?y }",
        "SELECT * WHERE { ?x <urn:p> ?y . ?x ?r ?y }",
        "SELECT * WHERE { ?x <urn:p> ?y . ?y ?x ?z }",
        "SELECT * WHERE { ?x <urn:p> ?y . ?z ?x ?y }",
        "SELECT * WHERE { ?x <urn:q> ?y . ?x <urn:p> ?z . ?z ?r ?z }",
        "SELECT * WHERE { ?x ?r <urn:a> . ?x ?r ?y }",
    ];
    for q in queries {
        let want = sorted(&scan, q);
        assert!(!want.is_empty(), "{q} must have answers");
        assert_eq!(sorted(&tail, q), want, "{q}");
    }
    assert!(tail.2.load(Ordering::Relaxed) > 0, "the tail path must be exercised");
}

/// `Options::count_pushdown = false` (the `--ablate=count` benchmark control) answers every count shape the index
/// shortcuts handle by full enumeration, with identical results: single patterns, joins (last-level counts),
/// disconnected parts (products), a cyclic pattern (leapfrog counts), DISTINCT and grouped counts.
#[test]
fn count_pushdown_off_gives_identical_answers() {
    let data = r#"<http://ex/a> <http://ex/p> <http://ex/b> .
<http://ex/b> <http://ex/p> <http://ex/c> .
<http://ex/c> <http://ex/p> <http://ex/a> .
<http://ex/a> <http://ex/p> <http://ex/c> .
<http://ex/a> <http://ex/q> "1" .
<http://ex/b> <http://ex/q> "1" .
<http://ex/c> <http://ex/q> "2" .
<http://ex/d> <http://ex/r> <http://ex/a> .
"#;
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let run_with = |q: &str, count_pushdown: bool| -> Vec<String> {
        let opts = triplelight::exec::Options { count_pushdown, ..Default::default() };
        match triplelight::exec::query_with(&s, q, &opts).unwrap() { QueryResult::Solutions { rows, .. } => rows.into_iter().map(|r| format!("{r:?}")).collect(), r => panic!("{r:?}") }
    };
    let queries = [
        "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }",
        "SELECT (COUNT(*) AS ?n) WHERE { ?s <http://ex/p> ?o }",
        "SELECT (COUNT(?o) AS ?n) (COUNT(DISTINCT ?o) AS ?d) WHERE { ?s <http://ex/q> ?o }",
        "SELECT (COUNT(*) AS ?n) WHERE { ?s <http://ex/p> ?o . ?o <http://ex/q> ?v }",
        "SELECT (COUNT(*) AS ?n) WHERE { ?s <http://ex/p> ?o . ?x <http://ex/r> ?y }",
        "SELECT (COUNT(*) AS ?n) WHERE { ?x <http://ex/p> ?y . ?y <http://ex/p> ?z . ?z <http://ex/p> ?x }",
        "SELECT (COUNT(*) AS ?n) WHERE { ?s <http://ex/none> ?o }",
        "SELECT ?v (COUNT(*) AS ?n) WHERE { ?s <http://ex/q> ?v } GROUP BY ?v",
    ];
    // Compare as bags: incidental row order must not decide parity.
    let bag = |q: &str, on: bool| { let mut v = run_with(q, on); v.sort(); v };
    for q in queries { assert_eq!(bag(q, true), bag(q, false), "{q}"); }
}

/// The `count_pushdown` switch really changes the path: with it on, a single-pattern COUNT is answered from index
/// counts without reading a triple; with it off, the same query scans the matching triples. Both give the known
/// typed answer.
#[test]
fn count_pushdown_switch_changes_the_access_path() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use triplelight::model::{Perm, TermId, TripleBatch};
    use triplelight::storage::{Dictionary, Snapshot, StorageError};
    struct Probe(MemorySnapshot, AtomicUsize, AtomicUsize);
    impl Dictionary for Probe {
        fn lookup(&self, t: &triplelight_rdf::Term) -> Result<Option<TermId>, StorageError> { self.0.lookup(t) }
        fn resolve(&self, id: TermId) -> Result<triplelight_rdf::Term, StorageError> { self.0.resolve(id) }
        fn with_term(&self, id: TermId, f: &mut dyn FnMut(triplelight_rdf::TermRef<'_>)) -> Result<(), StorageError> { self.0.with_term(id, f) }
    }
    impl Snapshot for Probe {
        fn scan<'a>(&'a self, p: Perm, pre: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
            Box::new(self.0.scan(p, pre).inspect(|b| if let Ok(b) = b { self.1.fetch_add(b.len(), Ordering::Relaxed); }))
        }
        fn count(&self, p: Perm, pre: &[TermId]) -> Result<u64, StorageError> { self.2.fetch_add(1, Ordering::Relaxed); self.0.count(p, pre) }
        fn seek(&self, p: Perm, pre: &[TermId], t: TermId) -> Result<Option<TermId>, StorageError> { self.0.seek(p, pre, t) }
    }
    let data = "<urn:a> <urn:p> <urn:b> .\n<urn:b> <urn:p> <urn:c> .\n<urn:c> <urn:p> <urn:a> .\n<urn:a> <urn:q> <urn:a> .\n";
    let q = "SELECT (COUNT(*) AS ?n) WHERE { ?s <urn:p> ?o }";
    let run = |count_pushdown: bool| {
        let s = Probe(MemorySnapshot::from_ntriples(data.as_bytes()).unwrap(), AtomicUsize::new(0), AtomicUsize::new(0));
        let opts = triplelight::exec::Options { count_pushdown, ..Default::default() };
        let rows = match triplelight::exec::query_with(&s, q, &opts).unwrap() { QueryResult::Solutions { rows, .. } => rows, r => panic!("{r:?}") };
        (rows, s.1.load(Ordering::Relaxed), s.2.load(Ordering::Relaxed))
    };
    let (on, on_rows, on_counts) = run(true);
    let (off, off_rows, _) = run(false);
    let three: triplelight_rdf::Term = triplelight_rdf::Literal::new_typed_literal("3", triplelight_rdf::vocab::xsd::INTEGER).into();
    assert_eq!(on, vec![vec![Some(three)]], "known typed answer");
    assert_eq!(on, off);
    assert_eq!(on_rows, 0, "pushdown on reads no triples");
    assert!(on_counts > 0, "pushdown on uses index counts");
    assert!(off_rows >= 3, "pushdown off scans the matching triples ({off_rows})");
}

/// ORDER BY with several keys under a LIMIT computes the later keys only for rows whose first key can reach the
/// first k (`sort_rows`): the answer must equal the first k rows of the unlimited sort, ties and unbound keys included.
#[test]
fn top_k_with_secondary_keys_matches_the_full_sort() {
    let mut nt = String::new();
    for i in 0..300u32 {
        // Many ties on ?v (i % 7), a secondary IRI key, and some rows without ?v.
        if i % 11 != 0 { nt.push_str(&format!("<http://ex/s{i}> <http://ex/v> \"{}\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n", i % 7)) }
        nt.push_str(&format!("<http://ex/s{i}> <http://ex/k> <http://ex/k{}> .\n", (i * 37) % 101));
    }
    let snap = MemorySnapshot::from_ntriples(nt.as_bytes()).unwrap();
    let ordered = |q: &str| -> Vec<Vec<String>> {
        match query(&snap, q).unwrap() {
            QueryResult::Solutions { rows, .. } => rows.into_iter().map(|r| r.into_iter().map(|t| t.map(|t| t.to_string()).unwrap_or("-".into())).collect()).collect(),
            r => panic!("{r:?}"),
        }
    };
    for order in ["DESC(?v) ?k ?s", "?v DESC(?k) ?s", "(?v * 2) ?s", "DESC(?v + 1) ?k"] {
        let q = |limit: &str| format!("SELECT ?s ?v ?k WHERE {{ ?s <http://ex/k> ?k OPTIONAL {{ ?s <http://ex/v> ?v }} }} ORDER BY {order} {limit}");
        let full = ordered(&q(""));
        for k in [1, 2, 10, 43, 299, 300, 301] {
            assert_eq!(ordered(&q(&format!("LIMIT {k}"))), full[..k.min(full.len())].to_vec(), "ORDER BY {order} LIMIT {k}");
            assert_eq!(ordered(&q(&format!("LIMIT {k} OFFSET 5"))), full[5.min(full.len())..(5 + k).min(full.len())].to_vec(), "ORDER BY {order} LIMIT {k} OFFSET 5");
        }
    }
}

/// `Options::interrupt` (#110): a cancelled or expired query ends with its own error kind and no partial answer,
/// a cancel from another thread stops a long query mid-run, and an untouched interrupt changes nothing.
#[test]
fn interrupt_cancels_and_times_out_running_queries() {
    use std::time::{Duration, Instant};
    use triplelight::exec::{query_with, Interrupt, Options, QueryErrorKind};
    let mut nt = String::new();
    for i in 0..3000u32 { nt.push_str(&format!("<http://ex/s{i}> <http://ex/p> \"{i}\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n")) }
    let snap = MemorySnapshot::from_ntriples(nt.as_bytes()).unwrap();
    // A cross product with a filter: about 9M rows to evaluate, far longer than the cancel delay below.
    let heavy = "SELECT (COUNT(*) AS ?n) WHERE { ?a <http://ex/p> ?x . ?b <http://ex/p> ?y FILTER(?x + ?y > -1) }";
    let run = |i: &Interrupt, q: &str| query_with(&snap, q, &Options { interrupt: Some(i), ..Options::default() });
    let cancelled = Interrupt::new();
    cancelled.cancel();
    assert_eq!(run(&cancelled, heavy).unwrap_err().kind, QueryErrorKind::Cancelled);
    let expired = Interrupt::with_timeout(Duration::ZERO);
    assert_eq!(run(&expired, heavy).unwrap_err().kind, QueryErrorKind::Timeout);
    let live = Interrupt::new();
    let started = Instant::now();
    let err = std::thread::scope(|scope| {
        scope.spawn(|| { std::thread::sleep(Duration::from_millis(50)); live.cancel() });
        run(&live, heavy).unwrap_err()
    });
    assert_eq!(err.kind, QueryErrorKind::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5), "stopped mid-run, not at the end: {:?}", started.elapsed());
    // An interrupt that never fires gives the same answer as none.
    let small = "SELECT ?a WHERE { ?a <http://ex/p> ?x FILTER(?x < 5) } ORDER BY ?a";
    let plain = query_with(&snap, small, &Options::default()).unwrap();
    let with = run(&Interrupt::with_timeout(Duration::from_secs(60)), small).unwrap();
    assert_eq!(format!("{plain:?}"), format!("{with:?}"));
}

/// `Options::memory_budget` (#110): a query whose working set does not fit ends with `ResourceLimit` instead of
/// growing, a budget that fits changes nothing, and every reservation is returned afterwards, also after the stop.
#[test]
fn memory_budget_stops_queries_that_do_not_fit_and_releases_everything() {
    use triplelight::exec::{query_with, MemoryBudget, Options, QueryErrorKind};
    let mut nt = String::new();
    for i in 0..2000u32 { nt.push_str(&format!("<http://ex/s{i}> <http://ex/p> \"{}\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n", i % 97)) }
    let snap = MemorySnapshot::from_ntriples(nt.as_bytes()).unwrap();
    let run = |b: &MemoryBudget, q: &str| query_with(&snap, q, &Options { memory_budget: Some(b), ..Options::default() });
    // A 4M-row cross product: far above a 1 MB budget.
    let cross = "SELECT ?a ?b WHERE { ?a <http://ex/p> ?x . ?b <http://ex/p> ?y }";
    let small = MemoryBudget::new(1 << 20);
    assert_eq!(run(&small, cross).unwrap_err().kind, QueryErrorKind::ResourceLimit);
    assert_eq!(small.used(), 0, "reservations are returned on the stop");
    // Grouping and sorting under a budget that fits give the same answer as without one, and return what they held.
    let q = "SELECT ?x (COUNT(*) AS ?n) WHERE { ?a <http://ex/p> ?x } GROUP BY ?x ORDER BY DESC(?n) ?x LIMIT 5";
    let roomy = MemoryBudget::new(64 << 20);
    let with = run(&roomy, q).unwrap();
    assert_eq!(format!("{with:?}"), format!("{:?}", query_with(&snap, q, &Options::default()).unwrap()));
    assert_eq!(roomy.used(), 0);
    // The reservation API on its own: charges add up, a charge past the limit fails, drops give back.
    let b = MemoryBudget::new(1000);
    let mut r = b.try_charge(600).unwrap();
    assert!(b.try_charge(500).is_err());
    r.try_resize(900).unwrap();
    assert_eq!(b.available(), 100);
    r.try_resize(100).unwrap();
    drop(r);
    assert_eq!(b.used(), 0);
}


/// #111: GROUP BY spilled to disk one key partition at a time gives the in-memory answer, rows and their order, on every
/// grouping path (flat single-key COUNT, the fast aggregates, the general path), and leaves no run files behind.
#[test]
fn spilled_group_by_matches_the_in_memory_answer_and_cleans_up() {
    use triplelight::exec::{phase, query_with, MemoryBudget, Options};
    let mut nt = String::new();
    for i in 0..3000u32 {
        nt.push_str(&format!("<http://ex/s{i}> <http://ex/p> \"{}\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n", (i * 7919) % 211));
        nt.push_str(&format!("<http://ex/s{i}> <http://ex/q> <http://ex/k{}> .\n", (i * 31) % 53));
    }
    let snap = MemorySnapshot::from_ntriples(nt.as_bytes()).unwrap();
    let scratch = std::env::temp_dir().join(format!("tl-spill-group-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let queries = [
        // flat single-key COUNT(*) and COUNT(?v), first-seen group order
        "SELECT ?k (COUNT(*) AS ?n) WHERE { ?s <http://ex/q> ?k } GROUP BY ?k",
        "SELECT ?k (COUNT(?v) AS ?n) WHERE { ?s <http://ex/q> ?k . ?s <http://ex/p> ?v } GROUP BY ?k",
        // fast aggregates, two keys
        "SELECT ?k ?v (SUM(?v) AS ?t) (MIN(?s) AS ?lo) (MAX(?s) AS ?hi) (AVG(?v) AS ?m) WHERE { ?s <http://ex/q> ?k . ?s <http://ex/p> ?v } GROUP BY ?k ?v",
        "SELECT ?k (COUNT(DISTINCT ?v) AS ?d) (SAMPLE(?v) AS ?x) WHERE { ?s <http://ex/q> ?k . ?s <http://ex/p> ?v } GROUP BY ?k",
        // the general path: an expression aggregate, GROUP_CONCAT, HAVING, and an ORDER BY on top
        "SELECT ?k (SUM(?v * 2) AS ?t) WHERE { ?s <http://ex/q> ?k . ?s <http://ex/p> ?v } GROUP BY ?k HAVING (SUM(?v) > 10)",
        "SELECT ?v (GROUP_CONCAT(STR(?k); SEPARATOR=\",\") AS ?ks) WHERE { ?s <http://ex/q> ?k . ?s <http://ex/p> ?v } GROUP BY ?v",
        "SELECT ?k (COUNT(*) AS ?n) WHERE { ?s <http://ex/q> ?k } GROUP BY ?k ORDER BY DESC(?n) ?k",
    ];
    phase::take_spilled();
    for q in queries {
        let want = format!("{:?}", query_with(&snap, q, &Options::default()).unwrap());
        for general_group in [false, true] {
            let budget = MemoryBudget::new(64 << 20).with_scratch_dir(&scratch).with_spill_rows(100);
            let got = query_with(&snap, q, &Options { memory_budget: Some(&budget), general_group, ..Options::default() }).unwrap();
            assert_eq!(format!("{got:?}"), want, "{q} (general_group {general_group})");
            assert_eq!(budget.used(), 0);
        }
    }
    assert!(phase::take_spilled() > 0, "the forced threshold made the groups spill");
    assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0, "every run directory is removed");
    std::fs::remove_dir_all(&scratch).unwrap();
}

/// #111: with a scratch directory, a GROUP BY whose groups would not fit next to its input finishes by spilling; the
/// same budget without one ends with `ResourceLimit`, as before.
#[test]
fn a_group_by_over_the_budget_spills_instead_of_failing() {
    use triplelight::exec::{query_with, MemoryBudget, Options, QueryErrorKind};
    let mut nt = String::new();
    for i in 0..40_000u32 { nt.push_str(&format!("<http://ex/s{i}> <http://ex/p> \"v{i}\" .\n")) }
    let snap = MemorySnapshot::from_ntriples(nt.as_bytes()).unwrap();
    // One group per row, on the general path: the groups hold every row again. HAVING keeps the answer small, so the
    // projection after it fits and only the grouping is over the budget.
    let q = "SELECT ?s (GROUP_CONCAT(?o) AS ?all) WHERE { ?s <http://ex/p> ?o } GROUP BY ?s HAVING (STRENDS(STR(?s), \"77\"))";
    let opts = |b| Options { memory_budget: Some(b), general_group: true, ..Options::default() };
    let budget = 3 << 20;
    let tight = MemoryBudget::new(budget);
    assert_eq!(query_with(&snap, q, &opts(&tight)).unwrap_err().kind, QueryErrorKind::ResourceLimit);
    let scratch = std::env::temp_dir().join(format!("tl-spill-limit-{}", std::process::id()));
    let spilling = MemoryBudget::new(budget).with_scratch_dir(&scratch);
    let got = query_with(&snap, q, &opts(&spilling)).unwrap();
    assert_eq!(format!("{got:?}"), format!("{:?}", query_with(&snap, q, &Options { general_group: true, ..Options::default() }).unwrap()));
    assert_eq!(spilling.used(), 0);
    assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0);
    std::fs::remove_dir_all(&scratch).unwrap();
}

#[test]
fn count_one_key_charges_its_group_table() {
    use triplelight::exec::{query_with, MemoryBudget, Options, QueryErrorKind};

    let values = (0..4096).map(|index| index.to_string()).collect::<Vec<_>>().join(" ");
    let q = format!(
        "SELECT ?key (COUNT(*) AS ?n) WHERE {{ VALUES ?key {{ {values} }} }} GROUP BY ?key"
    );
    let snapshot = store();
    let limited = MemoryBudget::new(4096 * 96);
    let opts = Options { memory_budget: Some(&limited), ..Default::default() };
    assert_eq!(query_with(&snapshot, &q, &opts).unwrap_err().kind, QueryErrorKind::ResourceLimit);
    assert_eq!(limited.used(), 0);

    let roomy = MemoryBudget::new(1 << 20);
    let opts = Options { memory_budget: Some(&roomy), ..Default::default() };
    let with = query_with(&snapshot, &q, &opts).unwrap();
    let without = query(&snapshot, &q).unwrap();
    assert_eq!(format!("{with:?}"), format!("{without:?}"));
    assert_eq!(roomy.used(), 0);
}
/// The numeric path for BIND / SELECT expressions and HAVING (#121) gives exactly the general evaluator's values:
/// each expression is compared with the same expression wrapped in `COALESCE`, which the numeric path does not
/// compile. Values cover integers, decimals, floats, doubles, zero divisors, non-numeric literals and unbound.
#[test]
fn numeric_extend_and_having_match_the_general_evaluator() {
    let mut nt = String::new();
    let lits = [
        "\"7\"^^<http://www.w3.org/2001/XMLSchema#integer>", "\"0\"^^<http://www.w3.org/2001/XMLSchema#integer>",
        "\"-3\"^^<http://www.w3.org/2001/XMLSchema#integer>", "\"2.50\"^^<http://www.w3.org/2001/XMLSchema#decimal>",
        "\"1.25E1\"^^<http://www.w3.org/2001/XMLSchema#double>", "\"0.1\"^^<http://www.w3.org/2001/XMLSchema#float>",
        "\"9223372036854775807\"^^<http://www.w3.org/2001/XMLSchema#integer>", "\"abc\"", "\"3\"",
        "\"NaN\"^^<http://www.w3.org/2001/XMLSchema#double>",
    ];
    for (i, a) in lits.iter().enumerate() {
        for (j, b) in lits.iter().enumerate() {
            nt.push_str(&format!("<http://ex/s{i}_{j}> <http://ex/a> {a} .\n<http://ex/s{i}_{j}> <http://ex/b> {b} .\n<http://ex/s{i}_{j}> <http://ex/g> <http://ex/g{} > .\n", (i + j) % 4).replace(" >", ">"));
        }
    }
    nt.push_str("<http://ex/lone> <http://ex/a> \"5\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n");
    let snap = MemorySnapshot::from_ntriples(nt.as_bytes()).unwrap();
    let rows = |q: &str| -> Vec<Vec<String>> {
        match query(&snap, q).unwrap() {
            QueryResult::Solutions { rows, .. } => { let mut v: Vec<Vec<String>> = rows.into_iter().map(|r| r.into_iter().map(|t| t.map(|t| t.to_string()).unwrap_or("-".into())).collect()).collect(); v.sort(); v }
            r => panic!("{r:?}"),
        }
    };
    let xsd = "PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> ";
    for e in ["?a / ?b", "xsd:float(?a) / ?b", "?a * ?b - 1", "xsd:float(?a)", "?a + 2.5", "(?a - ?b) / 3", "xsd:float(?a / ?b)"] {
        let fast = format!("{xsd}SELECT ?s ({e} AS ?r) WHERE {{ ?s <http://ex/a> ?a OPTIONAL {{ ?s <http://ex/b> ?b }} }}");
        let slow = format!("{xsd}SELECT ?s (COALESCE({e}) AS ?r) WHERE {{ ?s <http://ex/a> ?a OPTIONAL {{ ?s <http://ex/b> ?b }} }}");
        assert_eq!(rows(&fast), rows(&slow), "{e}");
    }
    for h in ["COUNT(*) > 20", "SUM(?a) > 10", "xsd:float(COUNT(?b)) / COUNT(*) >= 1", "MAX(?a) < 8 && COUNT(*) > 0"] {
        let fast = format!("{xsd}SELECT ?g (COUNT(*) AS ?n) WHERE {{ ?s <http://ex/g> ?g . ?s <http://ex/a> ?a OPTIONAL {{ ?s <http://ex/b> ?b }} }} GROUP BY ?g HAVING ({h})");
        let slow = format!("{xsd}SELECT ?g (COUNT(*) AS ?n) WHERE {{ ?s <http://ex/g> ?g . ?s <http://ex/a> ?a OPTIONAL {{ ?s <http://ex/b> ?b }} }} GROUP BY ?g HAVING (COALESCE({h}))");
        assert_eq!(rows(&fast), rows(&slow), "{h}");
    }
}

#[test]
fn named_blank_nodes_follow_each_solution_occurrence() {
    let q = r#"
        SELECT (BNODE(?label) AS ?a) (BNODE("same"^^<http://www.w3.org/2001/XMLSchema#string>) AS ?b)
               (BNODE("other") AS ?c) (BNODE() AS ?fresh)
               (BNODE("same"@en) AS ?bad)
        WHERE { VALUES ?label { "same" "same" } }
    "#;
    let QueryResult::Solutions { rows, .. } = query(&store(), q).unwrap() else {
        panic!("expected solution mappings");
    };
    assert_eq!(rows.len(), 2);
    for row in &rows {
        assert!(matches!(row[0], Some(triplelight::rdf::Term::BlankNode(_))));
        assert_eq!(row[0], row[1]);
        assert_ne!(row[0], row[2]);
        assert_ne!(row[0], row[3]);
        assert_eq!(row[4], None);
    }
    assert_ne!(rows[0][0], rows[1][0]);
    assert_ne!(rows[0][3], rows[1][3]);

    let mixed = r#"
        PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
        SELECT (BNODE("same") AS ?a) (xsd:float(?n) / ?d AS ?ratio)
               (BNODE("same") AS ?b)
        WHERE { VALUES (?n ?d) { (9 2) } }
    "#;
    let QueryResult::Solutions { rows, .. } = query(&store(), mixed).unwrap() else {
        panic!("expected a blank node and a float");
    };
    assert_eq!(rows.len(), 1);
    assert!(matches!(rows[0][0], Some(triplelight::rdf::Term::BlankNode(_))));
    assert_eq!(rows[0][0], rows[0][2]);
    let ratio = triplelight::rdf::Literal::new_typed_literal(
        "4.5",
        triplelight_rdf::vocab::xsd::FLOAT,
    );
    assert_eq!(rows[0][1], Some(ratio.into()));
    assert!(matches!(
        query(&store(), r#"ASK { FILTER(sameTerm(BNODE("x"), BNODE("x"))) }"#).unwrap(),
        QueryResult::Boolean(true)
    ));
}

/// GROUP BY one key with one COUNT over a BGP (optionally filtered inside it) counts the BGP's flat buffer in place
/// (`agg::flat`); groups, their order and their counts equal the general grouping's, row for row.
#[test]
fn flat_count_group_matches_general_group() {
    let mut data = String::new();
    for r in 0..300 {
        let (p, d) = (r * 7 % 23, 1 + r * 13 % 28);
        data.push_str(&format!("<http://ex/r{r}> <http://ex/for> <http://ex/p{p}> .\n"));
        data.push_str(&format!("<http://ex/r{r}> <http://ex/date> \"2008-02-{d:02}\"^^<http://www.w3.org/2001/XMLSchema#date> .\n"));
        if r % 3 == 0 { data.push_str(&format!("<http://ex/r{r}> <http://ex/for> <http://ex/p{}> .\n", p + 1)) }
    }
    let s = MemorySnapshot::from_ntriples(data.as_bytes()).unwrap();
    let run = |q: &str, general_group: bool| -> Vec<String> {
        let opts = triplelight::exec::Options { general_group, count_pushdown: false, ..Default::default() };
        match triplelight::exec::query_with(&s, q, &opts).unwrap() { QueryResult::Solutions { rows, .. } => rows.into_iter().map(|r| format!("{r:?}")).collect(), r => panic!("{r:?}") }
    };
    let date = "<http://www.w3.org/2001/XMLSchema#date>";
    let queries = [
        format!("SELECT ?p (COUNT(?r) AS ?n) WHERE {{ ?r <http://ex/for> ?p . ?r <http://ex/date> ?d FILTER(?d >= \"2008-02-05\"^^{date} && ?d < \"2008-02-20\"^^{date}) }} GROUP BY ?p"),
        format!("SELECT ?p (COUNT(*) AS ?n) WHERE {{ ?r <http://ex/for> ?p . ?r <http://ex/date> ?d FILTER(?d < \"2008-02-10\"^^{date}) }} GROUP BY ?p"),
        "SELECT ?p (COUNT(?d) AS ?n) WHERE { ?r <http://ex/for> ?p . ?r <http://ex/date> ?d } GROUP BY ?p".to_string(),
        "SELECT ?d (COUNT(?r) AS ?n) WHERE { ?r <http://ex/date> ?d } GROUP BY ?d".to_string(),
        "SELECT ?p (COUNT(?r) AS ?n) WHERE { ?r <http://ex/for> ?p . ?r <http://ex/none> ?d } GROUP BY ?p".to_string(),
        // Not taken by the flat path (a filter left after the BGP, DISTINCT, two aggregates); still equal.
        "SELECT ?p (COUNT(?r) AS ?n) WHERE { ?r <http://ex/for> ?p FILTER(BOUND(?zz) || true) } GROUP BY ?p".to_string(),
        "SELECT ?p (COUNT(DISTINCT ?r) AS ?n) WHERE { ?r <http://ex/for> ?p } GROUP BY ?p".to_string(),
        // The BI q3 shape: two grouped subqueries joined, HAVING, a ratio and top-k.
        format!("PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> SELECT ?p (xsd:float(?a)/?b AS ?x) WHERE {{ {{ SELECT ?p (COUNT(?r) AS ?a) WHERE {{ ?r <http://ex/for> ?p . ?r <http://ex/date> ?d FILTER(?d >= \"2008-02-14\"^^{date}) }} GROUP BY ?p }} {{ SELECT ?p (COUNT(?r) AS ?b) WHERE {{ ?r <http://ex/for> ?p . ?r <http://ex/date> ?d FILTER(?d < \"2008-02-14\"^^{date}) }} GROUP BY ?p HAVING (COUNT(?r) > 0) }} }} ORDER BY DESC(xsd:float(?a)/?b) ?p LIMIT 5"),
    ];
    for q in &queries {
        let (flat, general) = (run(q, false), run(q, true));
        assert!(!flat.is_empty() || q.contains("none"), "{q}");
        assert_eq!(flat, general, "{q}");
    }
}

#[test]
fn string_cast_uses_numeric_and_boolean_values() {
    let result = rows(r#"
        PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
        SELECT (xsd:string("  keep me  ") AS ?text)
               (xsd:string("0"^^xsd:boolean) AS ?boolean)
               (xsd:string("01.200"^^xsd:decimal) AS ?decimal)
               (xsd:string("1E0"^^xsd:float) AS ?float)
               (xsd:string("1E8"^^xsd:double) AS ?scientific)
               (STR("1E0"^^xsd:float) AS ?lexical)
        WHERE {}
    "#);
    assert_eq!(result, vec![vec![
        "\"  keep me  \"", "\"false\"", "\"1.2\"", "\"1\"", "\"1.0E8\"", "\"1E0\"",
    ]]);
}

#[test]
fn timezone_string_canonicalizes_zero_offset() {
    let result = rows(r#"
        PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
        SELECT (TZ(?date) AS ?zone) WHERE {
            VALUES ?date {
                "2024-01-01T00:00:00+00:00"^^xsd:dateTime
                "2024-01-01T00:00:00-00:00"^^xsd:dateTime
                "2024-01-01T00:00:00-05:00"^^xsd:dateTime
                "2024-01-01T00:00:00"^^xsd:dateTime
            }
        }
    "#);
    assert_eq!(result, vec![vec!["\"\""], vec!["\"-05:00\""], vec!["\"Z\""], vec!["\"Z\""]]);
}

#[test]
fn fractional_and_integral_decimal_function_results() {
    let result = rows(r#"
        PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
        SELECT (CEIL(1.1) AS ?ceiling)
               (FLOOR(-1.1) AS ?floor)
               (ROUND(-1.5) AS ?nearest)
               (SECONDS("2024-01-01T00:00:02Z"^^xsd:dateTime) AS ?seconds)
               (SECONDS("2024-01-01T00:00:02.500Z"^^xsd:dateTime) AS ?fractional)
        WHERE {}
    "#);
    let expected = ["2", "-2", "-1", "2", "2.5"].map(|value| {
        format!(r#""{value}"^^<http://www.w3.org/2001/XMLSchema#decimal>"#)
    });
    assert_eq!(result, vec![expected.to_vec()]);
}
