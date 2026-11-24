//! Property paths: multiplicity of fixed-length paths, set semantics of `*` / `+` / `?`, zero-length matches,
//! cycles and self-loops, and every combination of bound and unbound endpoints.
use triplelight::exec::{query, QueryResult};
use triplelight::storage::MemorySnapshot;

const DATA: &str = r#"
<http://ex/a> <http://ex/p> <http://ex/b> .
<http://ex/b> <http://ex/p> <http://ex/c> .
<http://ex/c> <http://ex/p> <http://ex/a> .
<http://ex/a> <http://ex/q> <http://ex/b> .
<http://ex/d> <http://ex/p> <http://ex/d> .
<http://ex/a> <http://ex/r> <http://ex/x1> .
<http://ex/a> <http://ex/r> <http://ex/x2> .
<http://ex/x1> <http://ex/s> <http://ex/y> .
<http://ex/x2> <http://ex/s> <http://ex/y> .
<http://ex/e> <http://ex/p> <http://ex/f> .
"#;

fn rows(q: &str) -> Vec<String> {
    let q = format!("PREFIX : <http://ex/> {q}");
    match query(&MemorySnapshot::from_ntriples(DATA.as_bytes()).unwrap(), &q).unwrap() {
        QueryResult::Solutions { rows, .. } => {
            let mut v: Vec<String> = rows.into_iter().map(|r| r.into_iter()
                .map(|t| t.map(|t| t.to_string().replace("http://ex/", "")).unwrap_or("-".into()))
                .collect::<Vec<_>>().join(" ")).collect();
            v.sort();
            v
        }
        r => panic!("{r:?}"),
    }
}

fn ask(q: &str) -> bool {
    let q = format!("PREFIX : <http://ex/> {q}");
    match query(&MemorySnapshot::from_ntriples(DATA.as_bytes()).unwrap(), &q).unwrap() {
        QueryResult::Boolean(b) => b,
        r => panic!("{r:?}"),
    }
}

fn v(xs: &[&str]) -> Vec<String> { let mut v: Vec<String> = xs.iter().map(|s| s.to_string()).collect(); v.sort(); v }

#[test]
fn one_or_more_follows_cycles_without_repeating() {
    assert_eq!(rows("SELECT ?o WHERE { :a :p+ ?o }"), v(&["<a>", "<b>", "<c>"]));
    assert_eq!(rows("SELECT ?s WHERE { ?s :p+ :a }"), v(&["<a>", "<b>", "<c>"]));
    assert_eq!(rows("SELECT ?o WHERE { :d :p+ ?o }"), v(&["<d>"]));
    assert_eq!(rows("SELECT ?o WHERE { :e :p+ ?o }"), v(&["<f>"]));
}

#[test]
fn zero_or_more_includes_the_start_even_outside_the_graph() {
    assert_eq!(rows("SELECT ?o WHERE { :a :p* ?o }"), v(&["<a>", "<b>", "<c>"]));
    assert_eq!(rows("SELECT ?o WHERE { :d :p* ?o }"), v(&["<d>"]));
    assert_eq!(rows("SELECT ?o WHERE { :zzz :p* ?o }"), v(&["<zzz>"]));
    assert_eq!(rows("SELECT ?o WHERE { :y :p? ?o }"), v(&["<y>"]));
}

#[test]
fn fixed_length_paths_keep_duplicates() {
    // Two different intermediates (x1, x2) give two solutions.
    assert_eq!(rows("SELECT ?o WHERE { :a :r/:s ?o }"), v(&["<y>", "<y>"]));
    // Alternative is a union: a reaches b by both :p and :q.
    assert_eq!(rows("SELECT ?o WHERE { :a :p|:q ?o }"), v(&["<b>", "<b>"]));
    // ...but a closure over the same path is a set.
    assert_eq!(rows("SELECT ?o WHERE { :a (:r/:s)* ?o }"), v(&["<a>", "<y>"]));
    assert_eq!(rows("SELECT ?o WHERE { :a (:p|:q)+ ?o }"), v(&["<a>", "<b>", "<c>"]));
}

#[test]
fn inverse_and_negated_sets() {
    assert_eq!(rows("SELECT ?x WHERE { :a ^:p ?x }"), v(&["<c>"]));
    assert_eq!(rows("SELECT ?x WHERE { :y ^:s/^:r ?x }"), v(&["<a>", "<a>"]));
    assert_eq!(rows("SELECT ?o WHERE { :a !:p ?o }"), v(&["<b>", "<x1>", "<x2>"]));
    assert_eq!(rows("SELECT ?o WHERE { :a !(:p|^:p) ?o }"), v(&["<b>", "<x1>", "<x2>"]));
    assert_eq!(rows("SELECT ?s WHERE { ?s !(:p|:q|:r) :y }"), v(&["<x1>", "<x2>"]));
}

#[test]
fn unbound_endpoints() {
    // Same variable at both ends: only nodes on a cycle.
    assert_eq!(rows("SELECT ?x WHERE { ?x :p+ ?x }"), v(&["<a>", "<b>", "<c>", "<d>"]));
    // Zero-length pairs for all 9 subjects/objects, plus the 4 :p edges that are not self-loops.
    assert_eq!(rows("SELECT ?s ?o WHERE { ?s :p? ?o }").len(), 13);
    assert_eq!(rows("SELECT ?s ?o WHERE { ?s :r/:s ?o }"), v(&["<a> <y>", "<a> <y>"]));
    assert_eq!(rows("SELECT ?s ?o WHERE { ?s :p+ ?o }").len(), 3 * 3 + 1 + 1);
}

#[test]
fn bound_endpoints_and_joins() {
    assert!(ask("ASK { :a :p+ :a }"));
    assert!(!ask("ASK { :e :p* :a }"));
    assert!(ask("ASK { :zzz :p* :zzz }"));
    assert_eq!(rows("SELECT * WHERE { :a :p+ :c }"), v(&[""]));
    // A path joined with an ordinary pattern, endpoints bound from the left.
    assert_eq!(rows("SELECT ?x ?o WHERE { ?x :q ?m . ?m :p+ ?o }"), v(&["<a> <a>", "<a> <b>", "<a> <c>"]));
}

#[test]
fn endpoints_bound_by_earlier_patterns_must_be_graph_nodes() {
    // W3C values_and_path: the path is evaluated on its own, so its zero-length matches are graph nodes only.
    assert_eq!(rows("SELECT * WHERE { VALUES ?v { 1 } ?v :p? ?v }"), Vec::<String>::new());
    assert_eq!(rows("SELECT ?v WHERE { VALUES ?v { :a } ?v :p? ?v }"), v(&["<a>"]));
    // Sequences of alternatives are split by the parser on a blank node (W3C pp31).
    assert_eq!(rows("SELECT ?t WHERE { :a (:p|:q)/(:p|:q) ?t }"), v(&["<c>", "<c>"]));
}

/// `Options::csr_paths` on and off give the same rows in the same order (the CSR keeps the seek BFS's discovery
/// order and the all-pairs start order, see `path.rs`), on random graphs with cycles, self-loops, two predicates and
/// nodes that occur only as objects.
#[test]
fn csr_paths_match_seeks_row_for_row() {
    use triplelight::exec::{query_with, Options};
    let ordered = |snap: &MemorySnapshot, q: &str, csr_paths: bool| -> Vec<String> {
        let q = format!("PREFIX : <http://ex/> {q}");
        match query_with(snap, &q, &Options { csr_paths, ..Default::default() }).unwrap() {
            QueryResult::Solutions { rows, .. } => rows.into_iter().map(|r| r.into_iter()
                .map(|t| t.map(|t| t.to_string()).unwrap_or("-".into())).collect::<Vec<_>>().join(" ")).collect(),
            QueryResult::Boolean(b) => vec![b.to_string()],
            r => panic!("{r:?}"),
        }
    };
    let queries = [
        "SELECT * WHERE { ?x :p+ ?y }",
        "SELECT * WHERE { ?x :p* ?y }",
        "SELECT * WHERE { ?x ^:p+ ?y }",
        "SELECT * WHERE { ?x (^:p)* ?y }",
        "SELECT * WHERE { ?x ^(:p+) ?y }",
        "SELECT * WHERE { ?x :p+ ?x }",
        "SELECT * WHERE { ?x :q* ?y }",
        "SELECT * WHERE { :n3 :p+ ?y }",
        "SELECT * WHERE { :n3 :p* ?y }",
        "SELECT * WHERE { ?x :p+ :n3 }",
        "SELECT * WHERE { ?x :p* :n3 }",
        "SELECT * WHERE { :n3 ^:p+ ?y }",
        "SELECT * WHERE { :n3 :p+ :n4 }",
        "SELECT * WHERE { :zzz :p* ?y }",
        "SELECT * WHERE { ?x :p* :zzz }",
        "SELECT * WHERE { :sink1 :p* ?y }",
        "ASK { :zzz :p* :zzz }",
        // Many bound closures: seeks first, then the CSR once they add up to a share of :p's edges.
        "SELECT * WHERE { ?x :q ?m . ?m :p+ ?y }",
        "SELECT * WHERE { ?x :q ?m . ?y ^:p* ?m }",
        "SELECT * WHERE { ?x :p+ ?m . ?m :p+ ?y }",
    ];
    for seed in [0x2545_f491_4f6c_dd1du64, 0x9e37_79b9_7f4a_7c15, 42] {
        let mut rng = seed;
        let mut next = |m: u64| { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; rng % m };
        let mut nt = String::new();
        let mut edge = |s: String, p: &str, o: String| nt.push_str(&format!("<http://ex/{s}> <http://ex/{p}> <http://ex/{o}> .\n"));
        for _ in 0..120 { let (s, o) = (next(40), next(40)); edge(format!("n{s}"), if next(3) == 0 { "q" } else { "p" }, format!("n{o}")) }
        for x in 0..5 { edge(format!("n{x}"), "p", format!("n{x}")) }
        for x in 0..6 { let s = next(40); edge(format!("n{s}"), "p", format!("sink{x}")) }
        let snap = MemorySnapshot::from_ntriples(nt.as_bytes()).unwrap();
        for q in queries {
            assert_eq!(ordered(&snap, q, true), ordered(&snap, q, false), "{q} (seed {seed:#x})");
        }
    }
}
