//! Tests of `Store::open_read_only` (#112 step A): nothing on disk changes, readers share the directory, writes are refused.
use super::*;
use triplelight_rdf::{Literal, NamedNode};

fn iri(s: &str) -> NamedNode { NamedNode::new(format!("http://ex/{s}")).unwrap() }
fn t(s: &str, o: &str) -> Triple { Triple::new(iri(s), iri("p"), Literal::new_simple_literal(o)) }
fn rows(store: &Store) -> usize { store.snapshot().scan(Perm::Spo, &[]).map(|b| b.unwrap().cols[0].len()).sum() }

struct Dir(PathBuf);
impl Dir {
    fn new(name: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("rdf-readonly-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        Dir(d)
    }
}
impl std::ops::Drop for Dir { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

/// A store with two committed records, a torn third record, and a stray `base-7` that `CURRENT` does not name.
fn damaged(dir: &Dir) {
    let store = Store::open(&dir.0, None, 1).unwrap();
    store.commit(&[(Change::Insert, t("a", "1"))]).unwrap();
    store.commit(&[(Change::Insert, t("b", "2"))]).unwrap();
    drop(store);
    let mut wal = OpenOptions::new().append(true).open(dir.0.join("wal.log")).unwrap();
    std::io::Write::write_all(&mut wal, b"+ <http://ex/c> <http://ex/p> \"3\"").unwrap(); // no newline, no `C`
    std::fs::create_dir(dir.0.join("base-7")).unwrap();
    std::fs::write(dir.0.join("text-9.v1"), b"stale text index").unwrap();
}

#[test]
fn a_read_only_open_changes_nothing_on_disk() {
    let dir = Dir::new("pristine");
    damaged(&dir);
    let wal_before = std::fs::read(dir.0.join("wal.log")).unwrap();
    let current_before = std::fs::read(dir.0.join("CURRENT")).unwrap();
    let reader = Store::open_read_only(&dir.0, 1).unwrap();
    assert_eq!(rows(&reader), 2, "the two complete records are visible, the torn one is not");
    drop(reader);
    assert_eq!(std::fs::read(dir.0.join("wal.log")).unwrap(), wal_before, "the torn tail is still there");
    assert_eq!(std::fs::read(dir.0.join("CURRENT")).unwrap(), current_before);
    assert!(dir.0.join("base-7").is_dir(), "a base that CURRENT does not name is not deleted");
    assert!(dir.0.join("text-9.v1").is_file(), "a text index of another generation is not swept");
    // A writer still recovers it, as before.
    let writer = Store::open(&dir.0, None, 1).unwrap();
    assert_eq!(rows(&writer), 2);
    assert!(!dir.0.join("base-7").exists());
    assert!(!dir.0.join("text-9.v1").exists());
    assert_ne!(std::fs::read(dir.0.join("wal.log")).unwrap(), wal_before);
}

#[test]
fn damage_before_a_valid_batch_is_refused_and_left_untouched() {
    let dir = Dir::new("midlog");
    let store = Store::open(&dir.0, None, 1).unwrap();
    for (s, o) in [("a", "1"), ("b", "2"), ("c", "3")] { store.commit(&[(Change::Insert, t(s, o))]).unwrap(); }
    drop(store);
    let wal_path = dir.0.join("wal.log");
    let mut bytes = std::fs::read(&wal_path).unwrap();
    let at = bytes.windows(7).position(|w| w == b"http://").expect("a triple in the first batch") + 8;
    bytes[at] ^= 0x01;      // flip one bit inside the first batch; two valid batches follow it
    std::fs::write(&wal_path, &bytes).unwrap();
    let current_before = std::fs::read(dir.0.join("CURRENT")).unwrap();
    let err = Store::open_read_only(&dir.0, 1).err().expect("a damaged log must be refused");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "{err}");
    assert_eq!(std::fs::read(&wal_path).unwrap(), bytes, "the damaged log is left exactly as it was");
    assert_eq!(std::fs::read(dir.0.join("CURRENT")).unwrap(), current_before);
}

#[test]
fn a_truncated_batch_is_ignored_and_the_file_is_unchanged() {
    // The crash shape #282 actually produces: a `B <len> <hash>` header followed by fewer than `len` bytes.
    let dir = Dir::new("truncbatch");
    let store = Store::open(&dir.0, None, 1).unwrap();
    store.commit(&[(Change::Insert, t("a", "1"))]).unwrap();
    store.commit(&[(Change::Insert, t("b", "2"))]).unwrap();
    drop(store);
    let wal_path = dir.0.join("wal.log");
    let mut bytes = std::fs::read(&wal_path).unwrap();
    let last = bytes.windows(3).rposition(|w| w == b"\nB ").expect("a batch header") + 1;
    let tail = bytes[last..].to_vec();
    bytes.extend_from_slice(&tail[..tail.len() / 2]);       // half of a third batch
    std::fs::write(&wal_path, &bytes).unwrap();
    let reader = Store::open_read_only(&dir.0, 1).unwrap();
    assert_eq!(rows(&reader), 2, "the two complete batches are visible; the truncated one is not");
    drop(reader);
    assert_eq!(std::fs::read(&wal_path).unwrap(), bytes, "the torn tail is still there");
    // A writer cuts it off, as before.
    let writer = Store::open(&dir.0, None, 1).unwrap();
    assert_eq!(rows(&writer), 2);
    assert!(std::fs::read(&wal_path).unwrap().len() < bytes.len());
}

#[test]
fn a_store_without_a_lock_file_says_so() {
    let dir = Dir::new("nolock");
    Store::open(&dir.0, None, 1).unwrap().commit(&[(Change::Insert, t("a", "1"))]).unwrap();
    std::fs::remove_file(dir.0.join("LOCK")).unwrap();
    let err = Store::open_read_only(&dir.0, 1).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    assert!(err.to_string().contains("no LOCK file") && err.to_string().contains(&dir.0.display().to_string()), "{err}");
    assert!(!dir.0.join("LOCK").exists(), "a read-only open never creates the lock file");
}

#[cfg(unix)]
#[test]
fn a_read_only_store_spills_outside_its_root_even_when_a_writer_left_scratch_behind() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Dir::new("spill");
    Store::open(&dir.0, None, 1).unwrap().commit(&[(Change::Insert, t("a", "1"))]).unwrap();
    std::fs::create_dir(dir.0.join(SCRATCH_DIR)).unwrap();      // what an earlier writer's regroup leaves behind
    let set = |mode| for p in [dir.0.join(SCRATCH_DIR), dir.0.clone()] { std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap() };
    set(0o555);
    let reader = Store::open_read_only(&dir.0, 1).unwrap();
    let spill = crate::derived_order::spill_dir_for_test(&dir.0.join("base-0"));
    set(0o755);
    assert!(!spill.starts_with(&dir.0), "a read-only store must not spill under its root, got {}", spill.display());
    assert!(spill.is_dir() || !spill.exists(), "the spill location is a directory the process can use (the system temp directory or TRIPLELIGHT_SCRATCH_DIR)");
    drop(reader);
    // Once the reader is gone, the same base spills into the store's own scratch directory again.
    assert!(crate::derived_order::spill_dir_for_test(&dir.0.join("base-0")).starts_with(&dir.0));
}

#[test]
fn readers_share_the_directory_and_exclude_a_writer_both_ways() {
    let dir = Dir::new("locks");
    Store::open(&dir.0, None, 1).unwrap().commit(&[(Change::Insert, t("a", "1"))]).unwrap();
    let (r1, r2) = (Store::open_read_only(&dir.0, 1).unwrap(), Store::open_read_only(&dir.0, 1).unwrap());
    assert_eq!((rows(&r1), rows(&r2)), (1, 1));
    assert_eq!(Store::open(&dir.0, None, 1).err().unwrap().kind(), std::io::ErrorKind::WouldBlock, "no writer while readers are open");
    drop((r1, r2));
    let writer = Store::open(&dir.0, None, 1).unwrap();
    assert_eq!(Store::open_read_only(&dir.0, 1).err().unwrap().kind(), std::io::ErrorKind::WouldBlock, "no reader while a writer is open");
    drop(writer);
}

#[test]
fn a_read_only_store_refuses_every_write_and_a_missing_store_is_not_created() {
    let dir = Dir::new("refuse");
    Store::open(&dir.0, None, 1).unwrap().commit(&[(Change::Insert, t("a", "1"))]).unwrap();
    let reader = Store::open_read_only(&dir.0, 1).unwrap();
    assert_eq!(reader.commit(&[(Change::Insert, t("b", "2"))]).unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(reader.compact().unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(rows(&reader), 1);
    let missing = std::env::temp_dir().join(format!("rdf-readonly-missing-{}", std::process::id()));
    assert_eq!(Store::open_read_only(&missing, 1).err().unwrap().kind(), std::io::ErrorKind::NotFound);
    assert!(!missing.exists());
}
