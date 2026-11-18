//! Durable updates: overlay correctness against a rebuilt snapshot, reopen, torn WAL, compaction, crash.
use oxrdf::{Literal, NamedNode, Triple};
use rdfdb::exec::{query, QueryResult};
use rdfdb::storage::MemorySnapshot;
use rdfdb::store::{Change, Store};
use std::collections::BTreeSet;

fn n(i: u64) -> NamedNode { NamedNode::new_unchecked(format!("http://ex/{i}")) }
fn t(s: u64, p: u64, o: u64) -> Triple { Triple::new(n(s), n(100 + p), n(o)) }

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("rdfdb-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn answers<S: rdfdb::storage::Snapshot>(s: &S, q: &str) -> Vec<String> {
    match query(s, q).unwrap() {
        QueryResult::Solutions { rows, .. } => { let mut v: Vec<String> = rows.iter().map(|r| format!("{r:?}")).collect(); v.sort(); v }
        QueryResult::Boolean(b) => vec![b.to_string()],
        _ => unreachable!(),
    }
}

const QUERIES: [&str; 5] = [
    "SELECT * WHERE { ?s ?p ?o }",
    "SELECT ?a ?c WHERE { ?a <http://ex/100> ?b . ?b <http://ex/101> ?c }",
    // cyclic: exercises leapfrog, i.e. Overlay::seek
    "SELECT ?a ?b ?c WHERE { ?a <http://ex/100> ?b . ?b <http://ex/100> ?c . ?c <http://ex/100> ?a }",
    "SELECT (COUNT(*) AS ?n) WHERE { ?s <http://ex/102> ?o }",
    "ASK { <http://ex/1> <http://ex/100> <http://ex/2> }",
];

#[test]
fn overlay_matches_rebuilt_snapshot() {
    let dir = tmp("overlay");
    let base: Vec<Triple> = (0..60).map(|i| t(i % 13, i % 3, (i * 7) % 13)).collect();
    std::fs::create_dir_all(&dir).unwrap();
    let init = dir.join("init.nt");
    std::fs::write(&init, base.iter().map(|x| format!("{x} .\n")).collect::<String>()).unwrap();
    let store = Store::open(dir.join("db"), Some(&init), 2).unwrap();
    let mut expect: BTreeSet<String> = base.iter().map(|x| x.to_string()).collect();
    let mut rng = 12345u64;
    for round in 0..40 {
        let mut batch = Vec::new();
        for _ in 0..5 {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let (s, p, o) = ((rng >> 33) % 15, (rng >> 40) % 3, (rng >> 48) % 15);
            let c = if (rng >> 20) % 3 == 0 { Change::Delete } else { Change::Insert };
            batch.push((c, t(s, p, o)));
        }
        // A new literal term exercises delta-allocated IDs.
        batch.push((Change::Insert, Triple::new(n(1), n(102), Literal::new_simple_literal(format!("v{round}")))));
        for (c, x) in &batch { if *c == Change::Insert { expect.insert(x.to_string()); } else { expect.remove(&x.to_string()); } }
        store.commit(&batch).unwrap();
        let rebuilt = MemorySnapshot::from_ntriples(expect.iter().map(|x| format!("{x} .\n")).collect::<String>().as_bytes()).unwrap();
        let snap = store.snapshot();
        for q in QUERIES { assert_eq!(answers(&*snap, q), answers(&rebuilt, q), "round {round}: {q}"); }
    }
    // Reopen replays the WAL; compaction then reopen gives the same state.
    drop(store);
    let rebuilt = MemorySnapshot::from_ntriples(expect.iter().map(|x| format!("{x} .\n")).collect::<String>().as_bytes()).unwrap();
    let store = Store::open(dir.join("db"), None, 2).unwrap();
    for q in QUERIES { assert_eq!(answers(&*store.snapshot(), q), answers(&rebuilt, q), "reopen: {q}"); }
    store.compact().unwrap();
    assert_eq!(store.snapshot().delta_len(), 0);
    drop(store);
    let store = Store::open(dir.join("db"), None, 2).unwrap();
    for q in QUERIES { assert_eq!(answers(&*store.snapshot(), q), answers(&rebuilt, q), "compacted: {q}"); }
}

#[test]
fn torn_wal_tail_is_ignored() {
    let dir = tmp("torn");
    let store = Store::open(&dir, None, 1).unwrap();
    store.commit(&[(Change::Insert, t(1, 0, 2))]).unwrap();
    drop(store);
    // A crash mid-append leaves an uncommitted record without its `C` line.
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).open(dir.join("wal.log")).unwrap();
    write!(f, "+ {} .\n+ <http://ex/9", t(3, 0, 4)).unwrap();
    drop(f);
    let store = Store::open(&dir, None, 1).unwrap();
    let s = store.snapshot();
    assert_eq!(answers(&*s, "ASK { <http://ex/1> <http://ex/100> <http://ex/2> }"), ["true"]);
    assert_eq!(answers(&*s, "ASK { <http://ex/3> <http://ex/100> <http://ex/4> }"), ["false"]);
    // And the store keeps working after truncating the tail.
    store.commit(&[(Change::Insert, t(5, 0, 6))]).unwrap();
    drop(store);
    let store = Store::open(&dir, None, 1).unwrap();
    assert_eq!(answers(&*store.snapshot(), "ASK { <http://ex/5> <http://ex/100> <http://ex/6> }"), ["true"]);
}

#[test]
fn sparql_update_operations() {
    let dir = tmp("update");
    let store = Store::open(&dir, None, 1).unwrap();
    store.update("INSERT DATA { <http://ex/1> <http://ex/100> <http://ex/2> . <http://ex/2> <http://ex/100> <http://ex/3> }").unwrap();
    store.update("DELETE { ?a <http://ex/100> ?b } INSERT { ?b <http://ex/101> ?a } WHERE { ?a <http://ex/100> ?b FILTER(?a = <http://ex/1>) }").unwrap();
    let s = store.snapshot();
    assert_eq!(answers(&*s, "ASK { <http://ex/1> <http://ex/100> <http://ex/2> }"), ["false"]);
    assert_eq!(answers(&*s, "ASK { <http://ex/2> <http://ex/101> <http://ex/1> }"), ["true"]);
    assert_eq!(answers(&*s, "ASK { <http://ex/2> <http://ex/100> <http://ex/3> }"), ["true"]);
    store.update("DELETE DATA { <http://ex/2> <http://ex/100> <http://ex/3> }").unwrap();
    assert_eq!(answers(&*store.snapshot(), "ASK { <http://ex/2> <http://ex/100> <http://ex/3> }"), ["false"]);
    // Insert then delete of the same triple within one request nets to nothing.
    store.update("INSERT DATA { <http://ex/7> <http://ex/100> <http://ex/8> } ; DELETE DATA { <http://ex/7> <http://ex/100> <http://ex/8> }").unwrap();
    assert_eq!(answers(&*store.snapshot(), "ASK { <http://ex/7> <http://ex/100> <http://ex/8> }"), ["false"]);
    assert_eq!(store.update("LOAD <http://ex/x>").unwrap_err().kind, rdfdb::exec::QueryErrorKind::Unsupported);
}

/// kill -9 a writer mid-stream; every commit it acknowledged must survive.
#[test]
fn acknowledged_commits_survive_sigkill() {
    let dir = tmp("crash");
    let bin = env!("CARGO_BIN_EXE_rdfdb");
    let mut child = std::process::Command::new(bin).args(["stress-commit", dir.to_str().unwrap(), "100000"])
        .stdout(std::process::Stdio::piped()).spawn().unwrap();
    use std::io::BufRead;
    let mut acked = 0u64;
    let mut lines = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
    while let Some(Ok(l)) = lines.next() {
        acked = l.strip_prefix("acked ").unwrap().parse().unwrap();
        if acked >= 200 { break }
    }
    child.kill().unwrap(); // SIGKILL on unix
    child.wait().unwrap();
    let store = Store::open(&dir, None, 1).unwrap();
    let s = store.snapshot();
    for i in 0..=acked {
        assert_eq!(answers(&*s, &format!("ASK {{ <http://ex/{i}> <http://ex/100> <http://ex/{}> }}", i + 1)), ["true"], "commit {i} lost");
    }
}

/// A compaction that crashed before switching CURRENT leaves a stray base; open ignores and removes it.
#[test]
fn crashed_compaction_leftovers_are_ignored() {
    let dir = tmp("compact-crash");
    let store = Store::open(&dir, None, 1).unwrap();
    store.commit(&[(Change::Insert, t(1, 0, 2))]).unwrap();
    drop(store);
    std::fs::create_dir(dir.join("base-1")).unwrap();
    std::fs::write(dir.join("base-1/base.nt"), "garbage that is not N-Triples").unwrap();
    let store = Store::open(&dir, None, 1).unwrap();
    assert!(!dir.join("base-1").exists());
    assert_eq!(answers(&*store.snapshot(), "ASK { <http://ex/1> <http://ex/100> <http://ex/2> }"), ["true"]);
    store.compact().unwrap();
    assert_eq!(std::fs::read_to_string(dir.join("CURRENT")).unwrap().trim(), "base-1");
    drop(store);
    let store = Store::open(&dir, None, 1).unwrap();
    assert!(!dir.join("base-0").exists());
    assert_eq!(answers(&*store.snapshot(), "ASK { <http://ex/1> <http://ex/100> <http://ex/2> }"), ["true"]);
}

/// Concurrent committers (group commit): every commit is visible after return and after reopen.
#[test]
fn concurrent_commits_are_all_durable() {
    let dir = tmp("group");
    let store = Store::open(&dir, None, 1).unwrap();
    std::thread::scope(|s| for th in 0..8u64 { let store = &store; s.spawn(move || {
        for i in 0..50u64 {
            let k = th * 1000 + i;
            store.commit(&[(Change::Insert, t(k, 0, k + 1))]).unwrap();
            let q = format!("ASK {{ <http://ex/{k}> <http://ex/100> <http://ex/{}> }}", k + 1);
            assert_eq!(answers(&*store.snapshot(), &q), ["true"]);
        }
    }); });
    drop(store);
    let store = Store::open(&dir, None, 1).unwrap();
    let n = answers(&*store.snapshot(), "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }");
    assert!(n[0].contains("value: \"400\""), "{n:?}");
}

#[test]
fn disk_base_replays_and_compacts_updates() {
    use rdfdb::storage::DiskBase;
    let dir = tmp("disk-base");
    let open = || Store::open_with(&dir, None, Box::new(DiskBase { workers: 1 })).unwrap();
    let store = open();
    store.commit(&[(Change::Insert, t(1, 0, 2)), (Change::Insert, t(3, 1, 4))]).unwrap();
    drop(store);
    let store = open();
    let before = store.snapshot();
    let expected = answers(&*before, QUERIES[0]);
    assert_eq!(expected.len(), 2);
    store.compact().unwrap();
    assert_eq!(answers(&*before, QUERIES[0]), expected);
    assert_eq!(answers(&*store.snapshot(), QUERIES[0]), expected);
    drop(before); drop(store);
    let store = open();
    assert_eq!(answers(&*store.snapshot(), QUERIES[0]), expected);
    store.commit(&[(Change::Delete, t(1, 0, 2))]).unwrap();
    drop(store);
    let store = open();
    assert_eq!(answers(&*store.snapshot(), QUERIES[0]).len(), 1);
    drop(store); std::fs::remove_dir_all(dir).unwrap();
}

/// Scans larger than one batch (4096 rows) merge base and delta lazily across batch boundaries.
#[test]
fn overlay_scan_streams_across_batches() {
    let dir = tmp("stream");
    std::fs::create_dir_all(&dir).unwrap();
    let base: Vec<Triple> = (0..10_000).map(|i| t(i, 0, i + 1)).collect();
    let init = dir.join("init.nt");
    std::fs::write(&init, base.iter().map(|x| format!("{x} .\n")).collect::<String>()).unwrap();
    let store = Store::open(dir.join("db"), Some(&init), 2).unwrap();
    let mut expect: BTreeSet<String> = base.iter().map(|x| x.to_string()).collect();
    let mut batch = Vec::new();
    for i in (0..10_000).step_by(7) { batch.push((Change::Delete, t(i, 0, i + 1))); expect.remove(&t(i, 0, i + 1).to_string()); }
    for i in (0..10_000).step_by(5) { batch.push((Change::Insert, t(i, 0, i + 2))); expect.insert(t(i, 0, i + 2).to_string()); }
    store.commit(&batch).unwrap();
    let snap = store.snapshot();
    // Every batch is sorted and the concatenation is the expected set, in key order.
    let mut seen = Vec::new();
    for b in rdfdb::storage::Snapshot::scan(&*snap, rdfdb::model::Perm::Pso, &[]) {
        let b = b.unwrap();
        assert!(b.len() <= 4096);
        for i in 0..b.len() { seen.push([b.cols[0][i], b.cols[1][i], b.cols[2][i]]); }
    }
    assert!(seen.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(seen.len(), expect.len());
    let rebuilt = MemorySnapshot::from_ntriples(expect.iter().map(|x| format!("{x} .\n")).collect::<String>().as_bytes()).unwrap();
    assert_eq!(answers(&*snap, QUERIES[0]), answers(&rebuilt, QUERIES[0]));
}

/// A base read error in the middle of a merged scan surfaces as an error (not a short result).
#[test]
fn overlay_scan_propagates_base_errors() {
    use rdfdb::model::{Perm, TermId, TripleBatch};
    use rdfdb::storage::{Dictionary, Snapshot, StorageError};
    use std::sync::Arc;
    struct Faulty(MemorySnapshot);
    impl Dictionary for Faulty {
        fn lookup(&self, t: &oxrdf::Term) -> Result<Option<TermId>, StorageError> { self.0.lookup(t) }
        fn resolve(&self, id: TermId) -> Result<oxrdf::Term, StorageError> { self.0.resolve(id) }
        fn with_term(&self, id: TermId, f: &mut dyn FnMut(oxrdf::TermRef<'_>)) -> Result<(), StorageError> { self.0.with_term(id, f) }
    }
    impl Snapshot for Faulty {
        fn scan<'a>(&'a self, p: Perm, pre: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
            // First batch fine, then a failure.
            Box::new(self.0.scan(p, pre).take(1).chain(std::iter::once(Err(StorageError::other("planted read failure")))))
        }
        fn count(&self, p: Perm, pre: &[TermId]) -> Result<u64, StorageError> { self.0.count(p, pre) }
        fn seek(&self, p: Perm, pre: &[TermId], t: TermId) -> Result<Option<TermId>, StorageError> { self.0.seek(p, pre, t) }
    }
    struct FaultyFormat;
    impl rdfdb::store::BaseFormat for FaultyFormat {
        fn write(&self, _: &std::path::Path, _: &dyn Snapshot) -> std::io::Result<()> { unimplemented!() }
        fn import(&self, dir: &std::path::Path, _: Option<&std::path::Path>) -> std::io::Result<()> { std::fs::create_dir(dir) }
        fn open(&self, _: &std::path::Path) -> std::io::Result<Arc<dyn Snapshot>> {
            let data: String = (0..9000).map(|i| format!("{} .\n", t(i, 0, i + 1))).collect();
            Ok(Arc::new(Faulty(MemorySnapshot::from_ntriples(data.as_bytes()).unwrap())))
        }
    }
    let dir = tmp("faulty");
    let store = Store::open_with(&dir, None, Box::new(FaultyFormat)).unwrap();
    store.commit(&[(Change::Insert, t(1, 1, 1))]).unwrap(); // non-empty delta forces the merge path
    let results: Vec<_> = Snapshot::scan(&*store.snapshot(), Perm::Spo, &[]).collect();
    assert!(results.last().unwrap().is_err(), "error must surface");
    let e = rdfdb::exec::query(&*store.snapshot(), "SELECT * WHERE { ?s ?p ?o }").unwrap_err();
    assert_eq!(e.kind, rdfdb::exec::QueryErrorKind::Storage);
    let dest = dir.join("failed-generation");
    let err = rdfdb::storage::write_disk_snapshot(&dest, &*store.snapshot()).unwrap_err();
    assert!(err.to_string().contains("planted read failure"));
    assert!(!dest.join("MANIFEST").exists());
}
