use oxrdf::{NamedNode, Term, Triple};
use rdfdb::{
    model::{Perm, TermId, TripleBatch},
    opt::{
        plan::{Slot, TriplePattern},
        sampling::*,
    },
    storage::{Dictionary, MemorySnapshot, Snapshot, StorageError},
};
use std::time::Duration;
fn graph() -> MemorySnapshot {
    let n = |x: &str| NamedNode::new(format!("urn:{x}")).unwrap();
    MemorySnapshot::from_triples([
        Triple::new(n("a"), n("p"), n("a")),
        Triple::new(n("a"), n("p"), n("b")),
        Triple::new(n("b"), n("p"), n("c")),
        Triple::new(n("a"), n("q"), n("c")),
        Triple::new(n("b"), n("q"), n("c")),
    ])
}
fn id(s: &dyn Snapshot, x: &str) -> TermId {
    s.lookup(&NamedNode::new(format!("urn:{x}")).unwrap().into())
        .unwrap()
        .unwrap()
}
fn opts() -> WalkOptions {
    WalkOptions {
        walks: 20000,
        max_probes: 200000,
        max_time: Duration::from_secs(10),
        ..Default::default()
    }
}
fn estimate(s: &dyn Snapshot, p: &[TriplePattern], order: &[usize]) -> WalkEstimate {
    let WalkOutcome::Complete(e) = estimate_walks(s, p, order, opts()).unwrap() else {
        panic!("expected complete sample")
    };
    e
}
#[test]
fn rank_matches_scan_all_permutations_and_prefixes() {
    let s = graph();
    for perm in Perm::ALL {
        let rows: Vec<_> = s
            .scan(perm, &[])
            .flat_map(|b| {
                let b = b.unwrap();
                (0..b.len()).map(move |i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]])
            })
            .collect();
        for row in &rows {
            for width in 0..=3 {
                let prefix = &row[..width];
                let selected: Vec<_> = rows
                    .iter()
                    .filter(|r| r.starts_with(prefix))
                    .copied()
                    .collect();
                for (k, &expected) in selected.iter().enumerate() {
                    assert_eq!(s.row_at(perm, prefix, k as u64).unwrap(), Some(expected));
                }
                assert_eq!(s.row_at(perm, prefix, selected.len() as u64).unwrap(), None);
                assert_eq!(s.row_at(perm, prefix, u64::MAX).unwrap(), None);
            }
        }
    }
}
#[test]
fn walks_preserve_bag_multiplicity_and_repeated_variable_constraints() {
    let s = graph();
    let p = id(&s, "p");
    let q = id(&s, "q");
    let patterns = [
        TriplePattern {
            s: Slot::Var(0),
            p: Slot::Const(p),
            o: Slot::Var(1),
        },
        TriplePattern {
            s: Slot::Var(0),
            p: Slot::Const(q),
            o: Slot::Const(id(&s, "c")),
        },
    ];
    for order in [&[0, 1][..], &[1, 0][..]] {
        let e = estimate(&s, &patterns, order);
        assert!((e.rows - 3.).abs() < 0.08, "{e:?}");
    }
    let repeated = [TriplePattern {
        s: Slot::Var(0),
        p: Slot::Const(p),
        o: Slot::Var(0),
    }];
    let e = estimate(&s, &repeated, &[0]);
    assert!((e.rows - 1.).abs() < 0.05, "{e:?}");
    let unit = estimate(&s, &[], &[]);
    assert_eq!(unit.rows, 1.);
}
#[test]
fn deadline_and_probe_limits_discard_partial_estimates() {
    let s = graph();
    let p = [TriplePattern {
        s: Slot::Var(0),
        p: Slot::Var(1),
        o: Slot::Var(2),
    }];
    for o in [
        WalkOptions {
            max_probes: 3,
            ..opts()
        },
        WalkOptions {
            max_time: Duration::ZERO,
            ..opts()
        },
    ] {
        assert!(matches!(
            estimate_walks(&s, &p, &[0], o).unwrap(),
            WalkOutcome::BudgetExceeded { .. }
        ));
    }
    assert!(estimate_walks(&s, &p, &[0, 0], opts()).is_err());
}
struct Broken;
impl Dictionary for Broken {
    fn lookup(&self, _: &Term) -> Result<Option<TermId>, StorageError> {
        unreachable!()
    }
    fn resolve(&self, _: TermId) -> Result<Term, StorageError> {
        unreachable!()
    }
    fn with_term(
        &self,
        _: TermId,
        _: &mut dyn FnMut(oxrdf::TermRef<'_>),
    ) -> Result<(), StorageError> {
        unreachable!()
    }
}
impl Snapshot for Broken {
    fn scan<'a>(
        &'a self,
        _: Perm,
        _: &[TermId],
    ) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
        unreachable!()
    }
    fn count(&self, _: Perm, _: &[TermId]) -> Result<u64, StorageError> {
        Err(StorageError::new(
            std::io::ErrorKind::PermissionDenied,
            "sampling read failure",
        ))
    }
    fn seek(&self, _: Perm, _: &[TermId], _: TermId) -> Result<Option<TermId>, StorageError> {
        unreachable!()
    }
    fn has_fast_rank(&self) -> bool {
        true
    }
}
#[test]
fn storage_error_is_not_zero_estimate() {
    let p = [TriplePattern {
        s: Slot::Var(0),
        p: Slot::Var(1),
        o: Slot::Var(2),
    }];
    let e = estimate_walks(&Broken, &p, &[0], opts()).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(e.to_string(), "sampling read failure");
}
#[cfg(unix)]
#[test]
fn disk_rank_matches_its_scan_after_dictionary_reencoding() {
    use rdfdb::storage::{DiskSnapshot, write_disk_snapshot};
    let s = graph();
    let dir = std::env::temp_dir().join(format!("rdfdb-sampling-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write_disk_snapshot(&dir, &s).unwrap();
    let d = DiskSnapshot::open(&dir).unwrap();
    for perm in Perm::ALL {
        let rows: Vec<_> = d
            .scan(perm, &[])
            .flat_map(|b| {
                let b = b.unwrap();
                (0..b.len()).map(move |i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]])
            })
            .collect();
        for row in &rows {
            for width in 0..=3 {
                let prefix = &row[..width];
                let selected: Vec<_> = rows
                    .iter()
                    .filter(|r| r.starts_with(prefix))
                    .copied()
                    .collect();
                for (k, &expected) in selected.iter().enumerate() {
                    assert_eq!(d.row_at(perm, prefix, k as u64).unwrap(), Some(expected));
                }
                assert_eq!(d.row_at(perm, prefix, selected.len() as u64).unwrap(), None);
            }
        }
    }
    drop(d);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn overlay_rank_uses_visible_additions_and_deletions_and_refuses_sampling() {
    use rdfdb::store::{Change, Store};
    let dir = std::env::temp_dir().join(format!("rdfdb-sampling-overlay-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::open(&dir, None, 1).unwrap();
    let n = |x: &str| NamedNode::new(format!("urn:{x}")).unwrap();
    let t = |x: &str| Triple::new(n(x), n("p"), n("o"));
    store
        .commit(&[(Change::Insert, t("a")), (Change::Insert, t("b"))])
        .unwrap();
    store.compact().unwrap();
    store
        .commit(&[(Change::Delete, t("a")), (Change::Insert, t("c"))])
        .unwrap();
    let snap = store.snapshot();
    assert!(!snap.has_fast_rank());
    assert!(matches!(
        estimate_walks(&*snap, &[], &[], opts()).unwrap(),
        WalkOutcome::UnsupportedRank
    ));
    for perm in Perm::ALL {
        let rows: Vec<_> = snap
            .scan(perm, &[])
            .flat_map(|b| {
                let b = b.unwrap();
                (0..b.len()).map(move |i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]])
            })
            .collect();
        assert_eq!(rows.len(), 2);
        for (k, &row) in rows.iter().enumerate() {
            assert_eq!(snap.row_at(perm, &[], k as u64).unwrap(), Some(row));
        }
        assert_eq!(snap.row_at(perm, &[], 2).unwrap(), None);
    }
    drop(snap);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}
