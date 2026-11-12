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
    let fast = rdfdb::exec::results::serialize_ids(&sol, &s);
    let slow = rdfdb::exec::results::serialize(&query(&s, q).unwrap()).1;
    assert_eq!(fast, slow);
}
