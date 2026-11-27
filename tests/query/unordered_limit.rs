//! An unordered LIMIT or ASK over one triple pattern stops its scan after enough solutions (#153). The answer must be
//! `min(k, n)` solutions of the full answer, as a bag, for every shape the early stop accepts.
use triplelight::exec::{query, QueryResult};
use triplelight::storage::MemorySnapshot;

fn store() -> MemorySnapshot {
    let mut nt = String::new();
    for i in 0..50 { nt += &format!("<http://ex/s{i}> <http://ex/p> <http://ex/o{}> .\n", i % 7) }
    for i in 0..5 { nt += &format!("<http://ex/n{i}> <http://ex/self> <http://ex/n{}> .\n", if i % 2 == 0 { i } else { i + 1 }) }
    nt += "<http://ex/s0> <http://ex/q> \"x\" .\n";
    MemorySnapshot::from_ntriples(nt.as_bytes()).unwrap()
}

fn rows(q: &str) -> Vec<Vec<String>> {
    match query(&store(), q).unwrap() {
        QueryResult::Solutions { rows, .. } => rows.into_iter().map(|r| r.into_iter().map(|t| t.map(|t| t.to_string()).unwrap_or("-".into())).collect()).collect(),
        r => panic!("{r:?}"),
    }
}

fn ask(q: &str) -> bool {
    match query(&store(), q).unwrap() { QueryResult::Boolean(b) => b, r => panic!("{r:?}") }
}

/// `limited` has `min(k, |full|)` rows, each of them a distinct-position member of `full` (bag containment).
fn assert_sub_bag(limited: &[Vec<String>], full: &[Vec<String>], k: usize) {
    assert_eq!(limited.len(), k.min(full.len()));
    let mut rest = full.to_vec();
    for r in limited {
        let at = rest.iter().position(|f| f == r).unwrap_or_else(|| panic!("{r:?} is not a solution"));
        rest.swap_remove(at);
    }
}

#[test]
fn unordered_limit_returns_k_solutions_of_the_full_answer() {
    for body in ["?s <http://ex/p> ?o", "?s ?p ?o", "?s <http://ex/p> <http://ex/o3>", "<http://ex/s0> ?p ?o", "?s ?p <http://ex/o1>"] {
        let full = rows(&format!("SELECT * WHERE {{ {body} }}"));
        for (k, off) in [(0, 0), (1, 0), (5, 0), (5, 3), (1000, 0), (3, 1000)] {
            let limited = rows(&format!("SELECT * WHERE {{ {body} }} OFFSET {off} LIMIT {k}"));
            assert_sub_bag(&limited, &full, k.min(full.len().saturating_sub(off)));
        }
        let projected = rows(&format!("SELECT ?s WHERE {{ {body} }} LIMIT 4"));
        let full_s = rows(&format!("SELECT ?s WHERE {{ {body} }}"));
        assert_sub_bag(&projected, &full_s, 4);
    }
}

#[test]
fn repeated_variables_and_absent_constants() {
    // `?x :self ?x` holds for the even n_i only: the scan must check both positions, not take the first k rows.
    let full = rows("SELECT ?x WHERE { ?x <http://ex/self> ?x }");
    assert_eq!(full.len(), 3);
    assert_sub_bag(&rows("SELECT ?x WHERE { ?x <http://ex/self> ?x } LIMIT 2"), &full, 2);
    assert_eq!(rows("SELECT ?x WHERE { ?x <http://ex/self> ?x } LIMIT 10").len(), 3);
    assert!(rows("SELECT * WHERE { ?s <http://ex/absent> ?o } LIMIT 3").is_empty());
}

#[test]
fn ask_stops_early_with_the_same_answer() {
    assert!(ask("ASK { ?s <http://ex/p> ?o }"));
    assert!(ask("ASK { ?s ?p ?o }"));
    assert!(ask("ASK { ?x <http://ex/self> ?x }"));
    assert!(!ask("ASK { ?s <http://ex/absent> ?o }"));
    assert!(!ask("ASK { <http://ex/s1> <http://ex/q> ?o }"));
    assert!(ask("ASK { <http://ex/s0> <http://ex/q> ?o }"));
}

#[test]
fn other_shapes_keep_the_full_evaluation() {
    // Two patterns, DISTINCT and FILTER are not taken by the early stop; their answers are unchanged.
    let join = rows("SELECT * WHERE { ?s <http://ex/p> ?o . ?s <http://ex/q> ?v } LIMIT 5");
    assert_eq!(join.len(), 1);
    assert_eq!(rows("SELECT DISTINCT ?o WHERE { ?s <http://ex/p> ?o } LIMIT 100").len(), 7);
    assert_eq!(rows("SELECT ?s WHERE { ?s <http://ex/p> ?o FILTER(?o = <http://ex/o2>) } LIMIT 100").len(), 7);
}
