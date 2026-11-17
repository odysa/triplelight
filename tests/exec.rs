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
