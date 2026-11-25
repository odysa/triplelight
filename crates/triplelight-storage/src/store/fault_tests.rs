//! Fault injection for the release gates (`docs/release-checklist.md`: WAL crash boundaries, compaction crash
//! boundaries, disk-full and I/O errors), task #116.
//!
//! A crash is a child process, this test binary run again on `child_process`, that ends with `std::process::exit` at
//! one armed point of the write path (`super::fault`): no destructor runs, no buffered write is flushed and the
//! directory lock goes with the process, as in a kill. The parent then reopens the directory with the ordinary API
//! and compares every graph with the states a fault-free reference store went through. A torn WAL is a real store's
//! directory with its log cut at every byte of the last record. A full disk is an `ENOSPC` returned at the point.
//! A process crash keeps what reached the page cache; losing unsynced data on power loss is what the byte-offset cuts
//! model.
use super::dataset_wal_tests::state;
use super::fault::{self, Mode};
use super::*;
use triplelight_rdf::{Literal, NamedNode};

type State = (Vec<String>, Vec<(String, Vec<String>)>);

/// Records committed before a crash, and the record a commit-phase crash happens in.
const RECORDS: u64 = 5;
const CRASH_AT: u64 = 3;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Format { NTriples, #[cfg(unix)] Disk }
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind { Default, Named }

fn iri(s: &str) -> NamedNode { NamedNode::new(format!("http://ex/{s}")).unwrap() }
fn t(k: u64) -> Triple { Triple::new(iri(&format!("s{k}")), iri("p"), Literal::new_simple_literal(k.to_string())) }

/// Record `k`: insert triple `k` and delete triple `k - 2`. In a `Named` run, records from `CRASH_AT` on go to named
/// graphs, so record `CRASH_AT` is the first to touch one and fences CURRENT.
fn record(kind: Kind, k: u64) -> Vec<DatasetChange> {
    let target = match kind {
        Kind::Named if k >= CRASH_AT => GraphTarget::Named(iri(&format!("g{}", k % 2))),
        _ => GraphTarget::Default,
    };
    let mut changes = vec![DatasetChange::Quad(target.clone(), Change::Insert, t(k))];
    if k >= 2 { changes.push(DatasetChange::Quad(GraphTarget::Default, Change::Delete, t(k - 2))) }
    changes
}

fn open(format: Format, dir: &Path) -> std::io::Result<Store> {
    match format {
        Format::NTriples => Store::open(dir, None, 1),
        #[cfg(unix)]
        Format::Disk => Store::open_with(dir, None, Box::new(crate::DiskBase { workers: 1 })),
    }
}

/// A fresh directory, unique to this call: tests run in parallel and several build the same reference.
fn scratch(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("tl-faults-{name}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// The states a fault-free store goes through: `states[k]` is after record `k - 1` (`states[0]` is empty), then the
/// state after one more record that the recovered store commits as a retry.
fn reference(format: Format, kind: Kind) -> Vec<State> {
    let dir = scratch(&format!("ref-{format:?}-{kind:?}"));
    let store = open(format, &dir).unwrap();
    let mut states = vec![state(&store.snapshot())];
    for k in 0..=RECORDS {
        store.commit_dataset(&record(kind, k)).unwrap();
        states.push(state(&store.snapshot()));
    }
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
    states
}

/// Checks a recovered directory: it opens, the base CURRENT names verifies, no other base is left, and a retried
/// commit and compaction work and survive a reopen.
fn recovered(format: Format, kind: Kind, dir: &Path, states: &[State]) -> State {
    let store = open(format, dir).unwrap_or_else(|e| panic!("reopen failed: {e}"));
    let got = state(&store.snapshot());
    let (generation, ..) = parse_current(&std::fs::read_to_string(dir.join("CURRENT")).unwrap()).unwrap().unwrap();
    let bases: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("base-")).collect();
    assert_eq!(bases, [format!("base-{generation}")], "stray bases after reopen");
    #[cfg(unix)]
    if format == Format::Disk { crate::DiskSnapshot::open(dir.join(format!("base-{generation}"))).unwrap().verify().unwrap() }
    // Retry from where the store is: commit what is missing, then one more record, and compact.
    let at = states.iter().position(|s| *s == got).unwrap_or_else(|| panic!("recovered a state no commit sequence had: {got:?}"));
    for k in at as u64..=RECORDS { store.commit_dataset(&record(kind, k)).unwrap(); }
    store.compact().unwrap();
    drop(store);
    let again = open(format, dir).unwrap();
    assert_eq!(state(&again.snapshot()), *states.last().unwrap(), "retry after recovery");
    got
}

/// What a child process runs (`TL_FAULT_CHILD` = `format kind phase point dir`). Init phase: arms `point` before
/// the open that creates the store. Commit phase: commits the records,
/// arming `point` before record `CRASH_AT` (`after-ack` exits once it is acknowledged). Compaction phase: commits them
/// all, then arms `point` and compacts. Prints `acked k` per acknowledged record and `unreached` if the point never
/// fired, which the parent treats as a broken test.
#[test]
fn child_process() {
    let Ok(spec) = std::env::var("TL_FAULT_CHILD") else { return };
    let words: Vec<&str> = spec.split(' ').collect();
    let &[format, kind, phase, point, dir] = words.as_slice() else { panic!("bad TL_FAULT_CHILD {spec:?}") };
    let format = match format { "ntriples" => Format::NTriples, #[cfg(unix)] "disk" => Format::Disk, _ => panic!("{format}") };
    let kind = if kind == "named" { Kind::Named } else { Kind::Default };
    if phase == "init" { fault::arm(point, Mode::Exit) }
    let store = open(format, Path::new(dir)).unwrap();
    if phase == "group" {
        // Writers commit concurrently until the parent kills the process; each acknowledged triple is printed.
        let out = std::sync::Mutex::new(());
        std::thread::scope(|scope| for writer in 0..8u64 { let (store, out) = (&store, &out); scope.spawn(move || {
            for i in 0..u64::MAX {
                let k = writer * 1_000_000 + i;
                store.commit_dataset(&[DatasetChange::Quad(GraphTarget::Default, Change::Insert, t(k))]).unwrap();
                let _held = out.lock().unwrap();
                println!("acked {k}");
            }
        }); });
    }
    for k in 0..RECORDS {
        if phase == "commit" && k == CRASH_AT && point != "after-ack" { fault::arm(point, Mode::Exit) }
        store.commit_dataset(&record(kind, k)).unwrap();
        println!("acked {k}");
        if phase == "commit" && k == CRASH_AT && point == "after-ack" { std::process::exit(fault::EXIT) }
    }
    if phase == "compact" {
        fault::arm(point, Mode::Exit);
        store.compact().unwrap();
    }
    println!("unreached");
}

/// Runs a child that crashes at `point`, and returns the highest acknowledged record.
fn crash(format: Format, kind: Kind, phase: &str, point: &str, dir: &Path) -> Option<u64> {
    let name = match format { Format::NTriples => "ntriples", #[cfg(unix)] Format::Disk => "disk" };
    let kind_name = if kind == Kind::Named { "named" } else { "default" };
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["store::fault_tests::child_process", "--exact", "--nocapture", "--test-threads", "1"])
        .env("TL_FAULT_CHILD", format!("{name} {kind_name} {phase} {point} {}", dir.display()))
        .output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let label = format!("{name} {kind_name} {phase} {point}");
    assert!(!stdout.contains("unreached"), "{label}: the fault point never fired");
    assert_eq!(out.status.code(), Some(fault::EXIT), "{label}: {stdout}{}", String::from_utf8_lossy(&out.stderr));
    stdout.lines().filter_map(|l| l.strip_prefix("acked ")).map(|k| k.parse().unwrap()).max()
}

fn formats() -> Vec<Format> {
    let mut v = vec![Format::NTriples];
    #[cfg(unix)]
    v.push(Format::Disk);
    v
}

/// WAL crash boundaries: before the append, after it but before the fsync, after the fsync but before the
/// acknowledgement, and after the acknowledgement; for named graphs also around the CURRENT fence the first
/// named-graph record writes. Every acknowledged record survives, and the crashed one is all or nothing.
#[test]
fn wal_crash_boundaries() {
    for format in formats() {
        for kind in [Kind::Default, Kind::Named] {
            let states = reference(format, kind);
            let mut points = vec![("wal-before-write", false), ("wal-after-write", true), ("wal-after-sync", true), ("after-ack", true)];
            if kind == Kind::Named {
                points.extend([("current-after-tmp", false), ("current-after-rename", false), ("fence-after-current", false)]);
            }
            for (point, may_survive) in points {
                let label = format!("{format:?} {kind:?} {point}");
                let dir = scratch(&format!("wal-{format:?}-{kind:?}-{point}"));
                let acked = crash(format, kind, "commit", point, &dir);
                assert_eq!(acked, Some(CRASH_AT - 1 + u64::from(point == "after-ack")), "{label}");
                let got = recovered(format, kind, &dir, &states);
                let before = &states[CRASH_AT as usize];
                let after = &states[CRASH_AT as usize + 1];
                match point {
                    // Durable before the crash: the record is there.
                    "wal-after-sync" | "after-ack" => assert_eq!(&got, after, "{label}"),
                    _ if may_survive => assert!(got == *before || got == *after, "{label}: partial record {got:?}"),
                    _ => assert_eq!(&got, before, "{label}"),
                }
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }
}

/// Compaction crash boundaries: after the new base is written, around the CURRENT switch (temporary file written,
/// renamed, directory not yet synced), and before or after the WAL truncation; with named graphs in an N-Triples
/// store, around the new WAL that carries them. The store recovers exactly the committed state on a verified base.
#[test]
fn compaction_crash_boundaries() {
    for format in formats() {
        for kind in [Kind::Default, Kind::Named] {
            let states = reference(format, kind);
            let carried = kind == Kind::Named && format == Format::NTriples;
            let points: &[&str] = if carried {
                &["compact-after-base", "compact-after-wal-tmp", "current-after-tmp", "current-after-rename", "after-wal-rename"]
            } else {
                &["compact-after-base", "current-after-tmp", "current-after-rename", "after-current", "compact-after-truncate"]
            };
            for point in points {
                let label = format!("{format:?} {kind:?} {point}");
                let dir = scratch(&format!("compact-{format:?}-{kind:?}-{point}"));
                assert_eq!(crash(format, kind, "compact", point, &dir), Some(RECORDS - 1), "{label}");
                assert_eq!(recovered(format, kind, &dir, &states), states[RECORDS as usize], "{label}");
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }
}

/// A snapshot taken before a successful compaction keeps seeing its state after the switch, also once the store has
/// committed more on the new base.
#[test]
fn snapshot_held_through_compaction() {
    for format in formats() {
        for kind in [Kind::Default, Kind::Named] {
            let dir = scratch(&format!("held-{format:?}-{kind:?}"));
            let store = open(format, &dir).unwrap();
            for k in 0..RECORDS { store.commit_dataset(&record(kind, k)).unwrap(); }
            let held = store.snapshot();
            let want = state(&held);
            store.compact().unwrap();
            store.commit_dataset(&record(kind, RECORDS)).unwrap();
            assert_eq!(state(&held), want, "{format:?} {kind:?}");
            assert_ne!(state(&store.snapshot()), want);
            drop(store);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// The WAL of a real store cut at every byte of its last record: recovery keeps every record before it, never part
/// of it, and cuts the file back to the last complete record. Junk after the last `C`, and a last record partly
/// zeroed by power loss, are torn tails too.
#[test]
fn torn_wal_at_every_byte_of_the_last_record() {
    for kind in [Kind::Default, Kind::Named] {
        let states = reference(Format::NTriples, kind);
        let dir = scratch(&format!("tear-{kind:?}"));
        let store = open(Format::NTriples, &dir).unwrap();
        let mut ends = vec![std::fs::metadata(dir.join("wal.log")).unwrap().len()];
        for k in 0..RECORDS {
            store.commit_dataset(&record(kind, k)).unwrap();
            ends.push(std::fs::metadata(dir.join("wal.log")).unwrap().len());
        }
        drop(store);
        let wal = std::fs::read(dir.join("wal.log")).unwrap();
        let current = std::fs::read(dir.join("CURRENT")).unwrap();
        let (start, end) = (ends[RECORDS as usize - 1], ends[RECORDS as usize]);
        let mut cuts: Vec<Vec<u8>> = (start..end).map(|len| wal[..len as usize].to_vec()).collect();
        for junk in [&b"+ <http://ex/s> <http://ex/p> \"x\" .\n"[..], b"garbage\n", b"C"] {
            let mut v = wal[..start as usize].to_vec();
            v.extend_from_slice(junk);
            cuts.push(v);
        }
        // Power loss in the middle of an unsynced append: the file has its full length, but a filesystem may show
        // zeros for blocks whose data never reached the disk, while a later block, its `C` included, did.
        for zeros in [1, 8, (end - start) as usize - 2] {
            let mut v = wal.clone();
            v[start as usize..start as usize + zeros].fill(0);
            cuts.push(v);
        }
        for cut in cuts {
            let copy = scratch(&format!("tear-{kind:?}-copy"));
            std::fs::create_dir_all(&copy).unwrap();
            std::fs::write(copy.join("CURRENT"), &current).unwrap();
            copy_dir(&dir.join("base-0"), &copy.join("base-0"));
            std::fs::write(copy.join("wal.log"), &cut).unwrap();
            let store = open(Format::NTriples, &copy).unwrap();
            assert_eq!(state(&store.snapshot()), states[RECORDS as usize - 1], "{kind:?} cut at {}", cut.len());
            drop(store);
            assert_eq!(std::fs::metadata(copy.join("wal.log")).unwrap().len(), start, "{kind:?} cut at {}", cut.len());
            let _ = std::fs::remove_dir_all(&copy);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Group commit writes several records with one fsync. If power is lost during it, the first of them can come back
/// zeroed while a later one's block, its `C` included, landed. Neither was acknowledged, so this is a torn tail:
/// open recovers the state before both, and does not refuse the store. The two records are appended and then made
/// durable by one flush, so they are one group commit and, since #114, one WAL batch. (Two records with an fsync each
/// are two batches, and damage to the first is then damage, not a tear: it was acknowledged before the second was
/// written. `store::wal_batch_tests` covers that case.)
#[test]
fn zeroed_record_inside_the_last_group_commit_is_a_torn_tail() {
    let states = reference(Format::NTriples, Kind::Default);
    let dir = scratch("group-zero");
    let store = open(Format::NTriples, &dir).unwrap();
    for k in 0..RECORDS - 2 { store.commit_dataset(&record(Kind::Default, k)).unwrap(); }
    let first = std::fs::metadata(dir.join("wal.log")).unwrap().len();
    store.append(&record(Kind::Default, RECORDS - 2), super::Count::Steps).unwrap();
    let (seq, _) = store.append(&record(Kind::Default, RECORDS - 1), super::Count::Steps).unwrap();
    store.flush_until(seq).unwrap();
    drop(store);
    let mut wal = std::fs::read(dir.join("wal.log")).unwrap();
    let second = wal.len() as u64;
    // Zero the start of the group's records (its first record), keeping the rest of the batch intact.
    let header = wal[first as usize..].iter().position(|&b| b == b'\n').unwrap() as u64 + 1;
    wal[(first + header) as usize..(first + header + (second - first - header) / 3) as usize].fill(0);
    std::fs::write(dir.join("wal.log"), &wal).unwrap();
    let store = open(Format::NTriples, &dir).unwrap_or_else(|e| panic!("a power-loss tail must not make the store unopenable: {e}"));
    assert_eq!(state(&store.snapshot()), states[RECORDS as usize - 2]);
    drop(store);
    assert_eq!(std::fs::metadata(dir.join("wal.log")).unwrap().len(), first);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A base format that fails like a full disk while writing a base: it leaves a partial directory and returns `ENOSPC`.
struct FullDisk(Box<dyn BaseFormat>);
impl FullDisk {
    fn check(dir: &Path) -> std::io::Result<()> {
        fault::hit("base-write").inspect_err(|_| {
            let _ = std::fs::create_dir(dir);
            let _ = std::fs::write(dir.join("partial"), b"half a base");
        })
    }
}
impl BaseFormat for FullDisk {
    fn write(&self, dir: &Path, src: &dyn Snapshot) -> std::io::Result<()> { Self::check(dir)?; self.0.write(dir, src) }
    fn import(&self, dir: &Path, nt: Option<&Path>) -> std::io::Result<()> { Self::check(dir)?; self.0.import(dir, nt) }
    fn open(&self, dir: &Path) -> std::io::Result<Arc<dyn Snapshot>> { self.0.open(dir) }
    fn holds_named_graphs(&self) -> bool { self.0.holds_named_graphs() }
    fn write_dataset(&self, dir: &Path, src: &Overlay) -> std::io::Result<()> { Self::check(dir)?; self.0.write_dataset(dir, src) }
    fn rewrite(&self, previous: &Path, dir: &Path, src: &Overlay, dataset: bool) -> std::io::Result<()> {
        Self::check(dir)?;
        self.0.rewrite(previous, dir, src, dataset)
    }
    fn open_dataset(&self, dir: &Path) -> std::io::Result<(Arc<dyn Snapshot>, Vec<(TermId, Arc<dyn Snapshot>)>)> { self.0.open_dataset(dir) }
}

fn open_full_disk(format: Format, dir: &Path) -> std::io::Result<Store> {
    let inner: Box<dyn BaseFormat> = match format {
        Format::NTriples => Box::new(NTriplesBase { workers: 1 }),
        #[cfg(unix)]
        Format::Disk => Box::new(crate::DiskBase { workers: 1 }),
    };
    Store::open_with(dir, None, Box::new(FullDisk(inner)))
}

fn is_enospc(e: &std::io::Error) -> bool { e.raw_os_error() == Some(28) || e.to_string().contains("os error 28") }

/// Disk full at the WAL append and at its fsync: the commit fails with an error naming the WAL, nothing is published,
/// later writes are refused until a reopen, and the reopened store has every acknowledged record and takes the retry.
#[test]
fn disk_full_at_the_wal() {
    for format in formats() {
        for kind in [Kind::Default, Kind::Named] {
            let states = reference(format, kind);
            for point in ["wal-before-write", "wal-after-write"] {
                let label = format!("{format:?} {kind:?} {point}");
                let dir = scratch(&format!("full-wal-{format:?}-{kind:?}-{point}"));
                let store = open(format, &dir).unwrap();
                for k in 0..CRASH_AT { store.commit_dataset(&record(kind, k)).unwrap(); }
                fault::arm(point, Mode::NoSpace);
                let err = store.commit_dataset(&record(kind, CRASH_AT)).unwrap_err();
                fault::disarm();
                assert!(is_enospc(&err) && err.to_string().contains("wal.log") && err.to_string().contains("reopen"), "{label}: {err}");
                assert_eq!(state(&store.snapshot()), states[CRASH_AT as usize], "{label}: a failed append was published");
                assert!(store.commit_dataset(&record(kind, CRASH_AT)).is_err(), "{label}: a write after a failed append");
                drop(store);
                let got = recovered(format, kind, &dir, &states);
                let (before, after) = (&states[CRASH_AT as usize], &states[CRASH_AT as usize + 1]);
                assert!(got == *before || (point == "wal-after-write" && got == *after), "{label}: {got:?}");
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }
}

/// Disk full while compaction writes the new base: the compaction fails before anything switches, the store keeps
/// taking commits, the partial base is removed, and a retry compacts.
#[test]
fn disk_full_while_writing_a_base() {
    for format in formats() {
        for kind in [Kind::Default, Kind::Named] {
            let states = reference(format, kind);
            let label = format!("{format:?} {kind:?}");
            let dir = scratch(&format!("full-base-{format:?}-{kind:?}"));
            let store = open_full_disk(format, &dir).unwrap();
            for k in 0..RECORDS { store.commit_dataset(&record(kind, k)).unwrap(); }
            fault::arm("base-write", Mode::NoSpace);
            let err = store.compact().unwrap_err();
            fault::disarm();
            assert!(is_enospc(&err), "{label}: {err}");
            assert_eq!(parse_current(&std::fs::read_to_string(dir.join("CURRENT")).unwrap()).unwrap().unwrap().0, 0, "{label}: no switch");
            store.commit_dataset(&record(kind, RECORDS)).unwrap();
            assert_eq!(state(&store.snapshot()), *states.last().unwrap(), "{label}");
            store.compact().unwrap();
            assert!(!dir.join("base-1").join("partial").exists(), "{label}: partial base kept");
            drop(store);
            assert_eq!(state(&open(format, &dir).unwrap().snapshot()), *states.last().unwrap(), "{label}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// Disk full while publishing CURRENT, at compaction's switch and at the named-graph fence. A failed switch poisons
/// the store until a reopen, which recovers the committed state; a failed fence refuses only that update.
#[test]
fn disk_full_while_publishing_current() {
    for format in formats() {
        for kind in [Kind::Default, Kind::Named] {
            let states = reference(format, kind);
            let label = format!("{format:?} {kind:?}");
            let dir = scratch(&format!("full-current-{format:?}-{kind:?}"));
            let store = open(format, &dir).unwrap();
            for k in 0..RECORDS { store.commit_dataset(&record(kind, k)).unwrap(); }
            fault::arm("current-before-write", Mode::NoSpace);
            let err = store.compact().unwrap_err();
            fault::disarm();
            assert!(is_enospc(&err), "{label}: {err}");
            assert!(store.commit_dataset(&record(kind, RECORDS)).is_err(), "{label}: a commit after a failed switch");
            drop(store);
            assert_eq!(recovered(format, kind, &dir, &states), states[RECORDS as usize], "{label}");
            let _ = std::fs::remove_dir_all(&dir);
        }
        // The fence: the first named-graph update fails, the store stays usable and the retry fences.
        let states = reference(format, Kind::Named);
        let dir = scratch(&format!("full-fence-{format:?}"));
        let store = open(format, &dir).unwrap();
        for k in 0..CRASH_AT { store.commit_dataset(&record(Kind::Named, k)).unwrap(); }
        fault::arm("current-before-write", Mode::NoSpace);
        let err = store.commit_dataset(&record(Kind::Named, CRASH_AT)).unwrap_err();
        fault::disarm();
        assert!(is_enospc(&err), "{format:?} fence: {err}");
        assert_eq!(state(&store.snapshot()), states[CRASH_AT as usize], "{format:?} fence");
        store.commit_dataset(&record(Kind::Named, CRASH_AT)).unwrap();
        assert!(parse_current(&std::fs::read_to_string(dir.join("CURRENT")).unwrap()).unwrap().unwrap().1.dataset, "{format:?} fence");
        drop(store);
        assert_eq!(state(&open(format, &dir).unwrap().snapshot()), states[CRASH_AT as usize + 1], "{format:?} fence");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Disk full, or a crash, while `--init` creates `base-0`: the open fails or the process ends, and the next open
/// starts over and creates a working store.
#[test]
fn creating_the_first_base_fails_then_retries() {
    for format in formats() {
        let dir = scratch(&format!("init-{format:?}"));
        fault::arm("base-write", Mode::NoSpace);
        let err = open_full_disk(format, &dir).err().expect("open with a full disk");
        fault::disarm();
        assert!(is_enospc(&err), "{format:?}: {err}");
        assert!(!dir.join("CURRENT").exists());
        let store = open(format, &dir).unwrap();
        store.commit_dataset(&record(Kind::Default, 0)).unwrap();
        drop(store);
        assert_eq!(state(&open(format, &dir).unwrap().snapshot()).0.len(), 1, "{format:?}");
        let _ = std::fs::remove_dir_all(&dir);
        // A crash after base-0 is written but before CURRENT names it: the next open starts over.
        let dir = scratch(&format!("init-crash-{format:?}"));
        assert_eq!(crash(format, Kind::Default, "init", "init-after-import", &dir), None);
        assert!(dir.join("base-0").exists() && !dir.join("CURRENT").exists());
        let store = open(format, &dir).unwrap();
        store.commit_dataset(&record(Kind::Default, 0)).unwrap();
        drop(store);
        assert_eq!(state(&open(format, &dir).unwrap().snapshot()).0.len(), 1, "{format:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() { copy_dir(&path, &target) } else { std::fs::copy(&path, &target).unwrap(); }
    }
}

/// Group commit under a kill (#105): eight writers commit concurrently, so batches carry several of them, and the
/// process is SIGKILLed mid-stream. Every commit acknowledged before the kill is there after reopening.
#[test]
fn concurrent_commits_survive_a_kill_mid_group() {
    use std::io::BufRead;
    let dir = scratch("group-kill");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["store::fault_tests::child_process", "--exact", "--nocapture", "--test-threads", "1"])
        .env("TL_FAULT_CHILD", format!("ntriples default group none {}", dir.display()))
        .stdout(std::process::Stdio::piped()).spawn().unwrap();
    let mut acked = Vec::new();
    let mut lines = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
    while let Some(Ok(line)) = lines.next() {
        if let Some(k) = line.strip_prefix("acked ") { acked.push(k.parse::<u64>().unwrap()) }
        if acked.len() >= 400 { break }
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let store = open(Format::NTriples, &dir).unwrap();
    let rows: std::collections::HashSet<String> = state(&store.snapshot()).0.into_iter().collect();
    for k in &acked {
        let tr = t(*k);
        assert!(rows.contains(&format!("{} {} {}", tr.subject, tr.predicate, tr.object)), "acknowledged commit {k} lost");
    }
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A writer that leaves (#105): after a batch of four, a lone writer's leader waits for four records that never come,
/// but only up to the gather's cap; its own batch of one then ends the waiting for every later commit.
#[test]
fn a_departed_writer_costs_one_capped_gather_at_most() {
    let dir = scratch("departed");
    let store = open(Format::NTriples, &dir).unwrap();
    store.commit_dataset(&record(Kind::Default, 0)).unwrap();
    {
        let mut w = store.wal.lock().unwrap();
        (w.last_batch, w.last_sync) = (4, std::time::Duration::from_secs(1));
    }
    let t = std::time::Instant::now();
    store.commit_dataset(&record(Kind::Default, 1)).unwrap();
    assert!(t.elapsed() < std::time::Duration::from_millis(500), "the gather waited {:?}", t.elapsed());
    assert_eq!(store.wal.lock().unwrap().last_batch, 1, "a batch of one ends the gather");
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A follower whose record a finished batch did not cover (#105): it wakes, finds its record not yet durable and
/// nobody leading, and leads the next batch itself instead of waiting.
#[test]
fn a_follower_left_out_of_a_batch_leads_the_next() {
    let dir = scratch("left-out");
    let store = std::sync::Arc::new(open(Format::NTriples, &dir).unwrap());
    store.commit_dataset(&record(Kind::Default, 0)).unwrap();
    // Stand in for a leader: hold the WAL lock while a writer enqueues and starts waiting.
    let w = store.wal.lock().unwrap();
    let writer = { let store = std::sync::Arc::clone(&store); std::thread::spawn(move || store.commit_dataset(&record(Kind::Default, 1))) };
    while store.pending.lock().unwrap().records == 0 { std::thread::yield_now() }
    // The stand-in's batch ends without the writer's record: publish only what was durable before, then let go.
    let durable = store.durable.load(std::sync::atomic::Ordering::SeqCst);
    store.publish_durable(durable, store.snapshot());
    drop(w);
    let t = std::time::Instant::now();
    writer.join().unwrap().unwrap();
    assert!(t.elapsed() < std::time::Duration::from_secs(1), "the follower waited {:?}", t.elapsed());
    assert!(store.durable.load(std::sync::atomic::Ordering::SeqCst) > durable);
    drop(store);
    let reopened = open(Format::NTriples, &dir).unwrap();
    assert_eq!(state(&reopened.snapshot()).0.len(), 2);
    drop(reopened);
    let _ = std::fs::remove_dir_all(&dir);
}
