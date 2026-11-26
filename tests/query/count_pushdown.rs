use triplelight_rdf::{NamedNode, Term, Triple};
use triplelight::{
    exec::{QueryErrorKind, QueryResult, query},
    model::{Perm, TermId, TripleBatch},
    storage::{Dictionary, MemorySnapshot, Snapshot, StorageError},
};
use std::sync::atomic::{AtomicUsize, Ordering};
fn n(x: &str) -> NamedNode {
    NamedNode::new(format!("urn:{x}")).unwrap()
}
fn graph() -> MemorySnapshot {
    MemorySnapshot::from_triples(
        [
            ("a", "p", "x"),
            ("a", "p", "y"),
            ("a", "p", "a"),
            ("b", "p", "x"),
            ("a", "q", "z"),
            ("b", "q", "z"),
            ("c", "q", "z"),
        ]
        .into_iter()
        .map(|(s, p, o)| Triple::new(n(s), n(p), n(o))),
    )
}
fn scalar<S: Snapshot>(s: &S, q: &str) -> u64 {
    let QueryResult::Solutions { rows, .. } = query(s, q).unwrap() else {
        panic!("expected solutions")
    };
    assert_eq!(rows.len(), 1);
    let Some(Term::Literal(v)) = &rows[0][0] else {
        panic!("expected count")
    };
    v.value().parse().unwrap()
}
struct IndexOnly {
    inner: MemorySnapshot,
    scans: AtomicUsize,
    fail_count: bool,
    fail_seek: bool,
}
impl Dictionary for IndexOnly {
    fn lookup(&self, t: &Term) -> Result<Option<TermId>, StorageError> {
        self.inner.lookup(t)
    }
    fn resolve(&self, id: TermId) -> Result<Term, StorageError> {
        self.inner.resolve(id)
    }
    fn with_term(
        &self,
        id: TermId,
        f: &mut dyn FnMut(triplelight_rdf::TermRef<'_>),
    ) -> Result<(), StorageError> {
        self.inner.with_term(id, f)
    }
}
impl Snapshot for IndexOnly {
    fn scan<'a>(
        &'a self,
        p: Perm,
        k: &[TermId],
    ) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
        self.scans.fetch_add(1, Ordering::Relaxed);
        self.inner.scan(p, k)
    }
    fn count(&self, p: Perm, k: &[TermId]) -> Result<u64, StorageError> {
        if self.fail_count {
            Err(StorageError::new(
                std::io::ErrorKind::PermissionDenied,
                "count probe failed",
            ))
        } else {
            self.inner.count(p, k)
        }
    }
    fn seek(&self, p: Perm, k: &[TermId], t: TermId) -> Result<Option<TermId>, StorageError> {
        if self.fail_seek {
            Err(StorageError::new(
                std::io::ErrorKind::PermissionDenied,
                "seek probe failed",
            ))
        } else {
            self.inner.seek(p, k, t)
        }
    }
}
fn indexed() -> IndexOnly {
    IndexOnly {
        inner: graph(),
        scans: AtomicUsize::new(0),
        fail_count: false,
        fail_seek: false,
    }
}
#[test]
fn single_pattern_counts_distinct_and_unbound_use_index_only() {
    let s = indexed();
    for (q, want) in [
        ("SELECT (COUNT(*) AS ?n) WHERE {?s ?p ?o}", 7),
        ("SELECT (COUNT(DISTINCT *) AS ?n) WHERE {?s <urn:p> ?o}", 4),
        ("SELECT (COUNT(?s) AS ?n) WHERE {?s <urn:p> ?o}", 4),
        ("SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE {?s <urn:p> ?o}", 2),
        ("SELECT (COUNT(DISTINCT ?o) AS ?n) WHERE {?s <urn:p> ?o}", 3),
        ("SELECT (COUNT(DISTINCT ?p) AS ?n) WHERE {?s ?p ?o}", 2),
        ("SELECT (COUNT(?missing) AS ?n) WHERE {?s ?p ?o}", 0),
        (
            "SELECT (COUNT(DISTINCT ?missing) AS ?n) WHERE {?s ?p ?o}",
            0,
        ),
        ("SELECT (COUNT(*) AS ?n) WHERE {?s <urn:absent> ?o}", 0),
        ("SELECT (COUNT(*) AS ?n) WHERE {<urn:a> <urn:p> <urn:x>}", 1),
        ("SELECT (COUNT(*) AS ?n) WHERE {<urn:z> <urn:p> <urn:x>}", 0),
        ("SELECT (COUNT(*) AS ?n) WHERE {}", 1),
    ] {
        assert_eq!(scalar(&s, q), want, "{q}");
    }
    assert_eq!(
        scalar(
            &s,
            "SELECT (COUNT(*) AS ?n) WHERE {?a <urn:p> ?b . ?x <urn:q> ?y}"
        ),
        12
    );
    assert_eq!(s.scans.load(Ordering::Relaxed), 0);
    assert_eq!(
        scalar(
            &s,
            "SELECT (COUNT(*) AS ?n) WHERE {?a <urn:p> ?b . ?a <urn:q> ?c}"
        ),
        4
    );
    // A star (any join on one variable) is counted from range counts too; a chain on two still scans.
    assert_eq!(s.scans.load(Ordering::Relaxed), 0);
    assert_eq!(
        scalar(
            &s,
            "SELECT (COUNT(*) AS ?n) WHERE {?a <urn:p> ?b . ?b <urn:q> ?c . ?c ?r ?d}"
        ),
        0
    );
    assert!(s.scans.load(Ordering::Relaxed) > 0);
}
#[test]
fn star_counts_match_the_evaluator() {
    // Subjects 0..40 with 0-3 values per predicate, objects shared, so centers in both positions, empty products and
    // repeated leaf values all occur.
    let mut triples = Vec::new();
    for s in 0..40u32 {
        for (k, p) in ["p", "q", "r"].iter().enumerate() {
            for j in 0..(s * (k as u32 + 3)) % 4 {
                triples.push(Triple::new(n(&format!("s{s}")), n(p), n(&format!("o{}", (s + j) % 7))));
            }
        }
        triples.push(Triple::new(n(&format!("o{}", s % 7)), n("t"), n(&format!("s{s}"))));
    }
    let snap = MemorySnapshot::from_triples(triples);
    let bgps = [
        "?c <urn:p> ?x . ?c <urn:q> ?y",
        "?c <urn:p> ?x . ?c <urn:q> ?y . ?c <urn:r> ?z",
        "?c <urn:p> <urn:o3> . ?c <urn:q> ?y",
        "?c <urn:p> ?x . ?c ?p ?y",
        "?c <urn:p> ?x . ?w <urn:t> ?c",
        "?x <urn:p> ?c . ?y <urn:q> ?c . ?c <urn:t> ?z",
        "?c <urn:p> ?x . ?c <urn:absent> ?y",
        "?c <urn:p> <urn:o1> . ?c <urn:q> <urn:o1>",
    ];
    for bgp in bgps {
        for agg in ["COUNT(*)", "COUNT(?c)", "COUNT(DISTINCT ?c)", "COUNT(DISTINCT ?x)", "COUNT(?y)"] {
            let pushed = format!("SELECT ({agg} AS ?n) WHERE {{ {bgp} }}");
            let evaluated = format!("SELECT ({agg} AS ?n) WHERE {{ {{ SELECT * WHERE {{ {bgp} }} }} }}");
            assert_eq!(scalar(&snap, &pushed), scalar(&snap, &evaluated), "{pushed}");
        }
    }
}
#[test]
fn prefix_groups_preserve_counts_duplicates_and_unbound_keys() {
    let s = indexed();
    let QueryResult::Solutions{vars,rows}=query(&s,"SELECT ?s (COUNT(*) AS ?n) (COUNT(DISTINCT ?o) AS ?d) (COUNT(DISTINCT ?s) AS ?one) WHERE {?s <urn:p> ?o} GROUP BY ?s").unwrap()else{unreachable!()};
    let col = |name: &str| vars.iter().position(|v| v.as_str() == name).unwrap();
    let mut values: Vec<_> = rows
        .iter()
        .map(|r| {
            (
                r[col("s")].as_ref().unwrap().to_string(),
                r[col("n")].as_ref().unwrap().to_string(),
                r[col("d")].as_ref().unwrap().to_string(),
                r[col("one")].as_ref().unwrap().to_string(),
            )
        })
        .collect();
    values.sort();
    assert_eq!(values.len(), 2);
    assert!(values[0].0.contains("urn:a"));
    for (row, n) in values.iter().zip([3, 1]) {
        assert_eq!(row.1, triplelight_rdf::Literal::from(n).to_string());
        assert_eq!(row.2, row.1);
        assert_eq!(row.3, triplelight_rdf::Literal::from(1).to_string());
    }
    let QueryResult::Solutions { rows, .. } = query(
        &s,
        "SELECT ?missing (COUNT(*) AS ?n) WHERE {?s <urn:p> ?o} GROUP BY ?missing",
    )
    .unwrap() else {
        unreachable!()
    };
    assert_eq!(rows.len(), 1);
    assert!(rows[0][0].is_none());
    let QueryResult::Solutions { rows, .. } = query(
        &s,
        "SELECT ?s (COUNT(*) AS ?n) WHERE {?s <urn:absent> ?o} GROUP BY ?s",
    )
    .unwrap() else {
        unreachable!()
    };
    assert!(rows.is_empty());
    assert_eq!(s.scans.load(Ordering::Relaxed), 0);
}
#[test]
fn factorized_counts_equal_materialized_bag_sizes_for_many_graphs() {
    for seed in 0..16u64 {
        let mut rng = seed + 1;
        let mut triples = Vec::new();
        for i in 0..60 {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            triples.push(Triple::new(
                n(&format!("v{}", rng % 7)),
                n(if i % 2 == 0 { "p" } else { "q" }),
                n(&format!("v{}", (rng >> 32) % 7)),
            ));
        }
        let s = MemorySnapshot::from_triples(triples);
        for bgp in [
            "?a <urn:p> ?b . ?b <urn:q> ?c",
            "?a <urn:p> ?b . ?b <urn:q> ?c . ?c <urn:p> ?a",
            "?a <urn:p> ?b . ?x <urn:q> ?y",
            "?a <urn:p> ?b . ?a <urn:p> ?b",
        ] {
            let QueryResult::Solutions { rows, .. } =
                query(&s, &format!("SELECT * WHERE {{{bgp}}}")).unwrap()
            else {
                unreachable!()
            };
            for agg in ["COUNT(*)", "COUNT(DISTINCT *)", "COUNT(?a)"] {
                assert_eq!(
                    scalar(&s, &format!("SELECT ({agg} AS ?n) WHERE {{{bgp}}}")),
                    rows.len() as u64,
                    "seed={seed} {agg} {bgp}"
                );
            }
        }
    }
}
#[test]
fn unsupported_algebra_and_repeated_variables_keep_semantics() {
    let s = graph();
    assert_eq!(
        scalar(&s, "SELECT (COUNT(*) AS ?n) WHERE {?x <urn:p> ?x}"),
        1
    );
    assert_eq!(
        scalar(
            &s,
            "SELECT (COUNT(*) AS ?n) WHERE {?s <urn:p> ?o FILTER(?s != ?o)}"
        ),
        3
    );
    assert_eq!(
        scalar(
            &s,
            "SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE {?s <urn:p> ?o . ?s <urn:q> ?z}"
        ),
        2
    );
    assert_eq!(
        scalar(
            &s,
            "SELECT (COUNT(*) AS ?n) WHERE {{SELECT DISTINCT ?s WHERE {?s <urn:p> ?o}}}"
        ),
        2
    );
}
#[test]
fn index_failures_do_not_become_zero_aggregates() {
    let mut s = indexed();
    s.fail_count = true;
    let err = query(&s, "SELECT (COUNT(*) AS ?n) WHERE {?s <urn:p> ?o}").unwrap_err();
    assert_eq!(err.kind, QueryErrorKind::Storage);
    assert!(err.message.contains("count probe failed"));
    let mut s = indexed();
    s.fail_seek = true;
    let err = query(
        &s,
        "SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE {?s <urn:p> ?o}",
    )
    .unwrap_err();
    assert_eq!(err.kind, QueryErrorKind::Storage);
    assert!(err.message.contains("seek probe failed"));
}

#[test]
fn counted_results_keep_projection_under_limit_and_distinct() {
    let s = graph();
    let QueryResult::Solutions { vars, rows } = query(
        &s,
        "SELECT DISTINCT (COUNT(*) AS ?n) WHERE {?s <urn:p> ?o} LIMIT 1",
    )
    .unwrap() else {
        unreachable!()
    };
    assert_eq!(vars.len(), 1);
    assert_eq!(vars[0].as_str(), "n");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Some(triplelight_rdf::Literal::from(4).into()));
    let QueryResult::Solutions { vars, rows } =
        query(&s, "SELECT (COUNT(*) AS ?n) WHERE {?s <urn:p> ?o} LIMIT 0").unwrap()
    else {
        unreachable!()
    };
    assert_eq!(vars.len(), 1);
    assert!(rows.is_empty());
}
#[test]
fn distinct_counts_preserve_literal_lexical_and_datatype_identity() {
    let one = triplelight_rdf::Literal::new_typed_literal("1", triplelight_rdf::vocab::xsd::INT);
    let zero_one = triplelight_rdf::Literal::new_typed_literal("01", triplelight_rdf::vocab::xsd::INT);
    let integer = triplelight_rdf::Literal::new_typed_literal("1", triplelight_rdf::vocab::xsd::INTEGER);
    let s = MemorySnapshot::from_triples([
        Triple::new(n("a"), n("p"), one.clone()),
        Triple::new(n("a"), n("p"), zero_one),
        Triple::new(n("a"), n("p"), integer),
        Triple::new(n("b"), n("p"), one),
    ]);
    assert_eq!(
        scalar(
            &s,
            "SELECT (COUNT(DISTINCT ?o) AS ?n) WHERE {?s <urn:p> ?o}"
        ),
        3
    );
    assert_eq!(
        scalar(&s, "SELECT (COUNT(*) AS ?n) WHERE {?s <urn:p> ?o}"),
        4
    );
}
/// A star of `?x p o` lists only is counted as the size of the lists' intersection (`count_star`, scan the smallest
/// and seek the rest): three lists of different sizes, in memory and on disk, small enough to stay on one thread and
/// large enough to split across workers, against a plain set intersection; an empty list gives zero.
#[test]
fn a_star_of_center_lists_counts_their_intersection() {
    use std::collections::BTreeSet;
    for subjects in [3_000u64, 100_000] {
        let (a, b, c) = (|x: u64| x % 2 == 0, |x: u64| x % 3 == 0, |x: u64| x % 5 != 1);
        let mut nt = String::new();
        for x in 0..subjects {
            if a(x) { nt.push_str(&format!("<urn:s{x}> <urn:a> <urn:A> .\n")) }
            if b(x) { nt.push_str(&format!("<urn:s{x}> <urn:b> <urn:B> .\n")) }
            if c(x) { nt.push_str(&format!("<urn:s{x}> <urn:c> <urn:C> .\n")) }
        }
        nt.push_str("<urn:E> <urn:b> <urn:B> .\n"); // urn:E exists, but nothing has `urn:a urn:E`
        let memory = MemorySnapshot::from_ntriples(nt.as_bytes()).unwrap();
        let dir = std::env::temp_dir().join(format!("triplelight-star-lists-{subjects}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        triplelight::storage::write_disk_snapshot(&dir, &memory).unwrap();
        let disk = triplelight::storage::DiskSnapshot::open(&dir).unwrap();
        let set = |f: &dyn Fn(u64) -> bool| (0..subjects).filter(|&x| f(x)).collect::<BTreeSet<u64>>();
        let want = set(&a).intersection(&set(&b)).filter(|&&x| c(x)).count() as u64;
        let q = "SELECT (COUNT(*) AS ?n) WHERE { ?x <urn:c> <urn:C> . ?x <urn:a> <urn:A> . ?x <urn:b> <urn:B> }";
        let distinct = "SELECT (COUNT(DISTINCT ?x) AS ?n) WHERE { ?x <urn:a> <urn:A> . ?x <urn:c> <urn:C> . ?x <urn:b> <urn:B> }";
        let empty = "SELECT (COUNT(*) AS ?n) WHERE { ?x <urn:a> <urn:E> . ?x <urn:b> <urn:B> }";
        fn check<S: Snapshot>(snap: &S, name: &str, queries: [(&str, u64); 3]) {
            for (q, want) in queries { assert_eq!(scalar(snap, q), want, "{name}: {q}") }
        }
        let queries = [(q, want), (distinct, want), (empty, 0)];
        check(&memory, &format!("memory {subjects}"), queries);
        check(&disk, &format!("disk {subjects}"), queries);
        drop(disk);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
