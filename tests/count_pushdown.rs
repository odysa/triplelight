use oxrdf::{NamedNode, Term, Triple};
use rdfdb::{
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
        f: &mut dyn FnMut(oxrdf::TermRef<'_>),
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
    assert_eq!(
        scalar(
            &s,
            "SELECT (COUNT(*) AS ?n) WHERE {?a <urn:p> ?b . ?a <urn:q> ?c}"
        ),
        4
    );
    assert_eq!(s.scans.load(Ordering::Relaxed), 0);
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
        assert_eq!(row.1, oxrdf::Literal::from(n).to_string());
        assert_eq!(row.2, row.1);
        assert_eq!(row.3, oxrdf::Literal::from(1).to_string());
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
