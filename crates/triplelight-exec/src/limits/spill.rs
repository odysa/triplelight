//! Spilling a blocking operator's input to disk under the memory budget (#111). Rows are written to run files under
//! `MemoryBudget::scratch_dir()`, hash-partitioned on the operator's key columns, each with its input position (`seq`),
//! so the operator can work one partition at a time and restore the order the in-memory path would produce.
//!
//! Each record is `seq` then the row's IDs, as little-endian `u64`s. The run files live in a fresh directory that
//! `Partitions` removes when it drops, which also happens when a cancelled or over-budget query unwinds. A process that
//! is killed cannot remove it, so each directory holds a `LOCK` file locked by its owner for as long as it exists, and
//! `sweep_stale` (run by the server at startup) removes the directories whose lock nobody holds.
use crate::model::TermId;
use std::fs::File;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// The rows of one operator's input, split by key hash into run files.
pub(crate) struct Partitions {
    dir: PathBuf,
    /// Held (locked) while the directory exists: `sweep_stale` leaves it alone.
    _lock: File,
    width: usize,
    rows: Vec<u64>,
    bytes: u64,
}

impl Partitions {
    /// Writes `rows` (each `width` IDs) into `parts` run files under a new directory in `scratch`, routed by a hash of
    /// the IDs at `key_cols`, through one `buffer`-byte writer per file (the caller charges `parts * buffer`). `seq` is
    /// each row's position in `rows`.
    pub(crate) fn write<'r>(
        scratch: &Path,
        width: usize,
        key_cols: &[usize],
        (parts, buffer): (usize, usize),
        rows: impl IntoIterator<Item = &'r [TermId]>,
        mut check: impl FnMut(),
    ) -> io::Result<Partitions> {
        let (dir, lock) = fresh_dir(scratch)?;
        // From here on the directory is owned, so an error or unwind below still removes it.
        let mut out = Partitions { dir, _lock: lock, width, rows: vec![0; parts], bytes: 0 };
        let mut files = Vec::with_capacity(parts);
        for p in 0..parts { files.push(BufWriter::with_capacity(buffer, File::create(out.path(p))?)); }
        let hasher = std::hash::RandomState::new();
        let mut record = Vec::with_capacity(8 * (width + 1));
        for (seq, row) in rows.into_iter().enumerate() {
            if seq % 4096 == 0 { check() }
            let mut h = hasher.build_hasher();
            for &c in key_cols { h.write_u64(row[c].0) }
            let p = (h.finish() % parts as u64) as usize;
            record.clear();
            record.extend_from_slice(&(seq as u64).to_le_bytes());
            for id in row { record.extend_from_slice(&id.0.to_le_bytes()) }
            files[p].write_all(&record)?;
            out.rows[p] += 1;
            out.bytes += record.len() as u64;
        }
        // Scratch runs need no durability: flush (keeping its error), no fsync.
        for f in files { f.into_inner().map_err(|e| e.into_error())?; }
        Ok(out)
    }
    pub(crate) fn count(&self) -> usize { self.rows.len() }
    /// Rows in partition `p`.
    pub(crate) fn rows(&self, p: usize) -> u64 { self.rows[p] }
    pub(crate) fn bytes(&self) -> u64 { self.bytes }
    /// The rows of partition `p` with their input positions, in input order.
    pub(crate) fn read(&self, p: usize) -> io::Result<(Vec<u64>, Vec<Vec<TermId>>)> {
        let n = self.rows[p] as usize;
        let mut file = BufReader::with_capacity(1 << 16, File::open(self.path(p))?);
        let (mut seqs, mut rows) = (Vec::with_capacity(n), Vec::with_capacity(n));
        let mut buf = vec![0u8; 8 * (self.width + 1)];
        for _ in 0..n {
            file.read_exact(&mut buf)?;
            let mut words = buf.as_chunks::<8>().0.iter().map(|b| u64::from_le_bytes(*b));
            seqs.push(words.next().unwrap());
            rows.push(words.map(TermId).collect());
        }
        Ok((seqs, rows))
    }
    fn path(&self, p: usize) -> PathBuf { self.dir.join(format!("part-{p}")) }
}

impl Drop for Partitions {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.dir); }
}

const PREFIX: &str = ".rdf-spill-";
const NEW_PREFIX: &str = ".rdf-spill-new-";

/// A directory of its own under `scratch`, unique within this process and across processes sharing `scratch`, with its
/// `LOCK` held. It is made under a `NEW_PREFIX` name and renamed once locked, so `sweep_stale` never sees it unlocked.
fn fresh_dir(scratch: &Path) -> io::Result<(PathBuf, File)> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::fs::create_dir_all(scratch)?;
    loop {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = format!("{}-{n}", std::process::id());
        let new = scratch.join(format!("{NEW_PREFIX}{name}"));
        match std::fs::create_dir(&new) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
        let locked = (|| {
            let lock = File::create(new.join("LOCK"))?;
            lock.try_lock().map_err(|e| match e { std::fs::TryLockError::Error(e) => e, std::fs::TryLockError::WouldBlock => io::Error::other("fresh spill LOCK is held") })?;
            let dir = scratch.join(format!("{PREFIX}{name}"));
            std::fs::rename(&new, &dir)?;
            Ok((dir, lock))
        })();
        if locked.is_err() { let _ = std::fs::remove_dir_all(&new); }
        return locked;
    }
}

/// Removes the spill directories under `scratch` that no live process owns: a `.rdf-spill-*` directory whose `LOCK`
/// can be taken (its owner exited, normally or killed), and `.rdf-spill-new-*` leftovers of a crash while one was being
/// made that are over an hour old. Returns how many it removed; a missing `scratch` removes none.
pub fn sweep_stale(scratch: &Path) -> io::Result<usize> {
    let entries = match std::fs::read_dir(scratch) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let path = entry.path();
        let stale = if name.starts_with(NEW_PREFIX) {
            entry.metadata()?.modified()?.elapsed().is_ok_and(|age| age > std::time::Duration::from_secs(3600))
        } else if name.starts_with(PREFIX) && path.is_dir() {
            // An owner holds the lock; a directory without one (only possible from an older build) is stale too.
            match File::open(path.join("LOCK")) {
                Ok(lock) => lock.try_lock().is_ok(),
                Err(e) if e.kind() == io::ErrorKind::NotFound => true,
                Err(e) => return Err(e),
            }
        } else { false };
        if stale && std::fs::remove_dir_all(&path).is_ok() { removed += 1 }
    }
    Ok(removed)
}

/// The most run files written at once: well under the usual open-file limit (256 on macOS), and with the write buffers
/// below, at most `MAX_PARTITIONS * 64 KiB` = 8 MiB of buffers. A partition that still does not fit ends its query with
/// `ResourceLimit` when it is read back.
pub(crate) const MAX_PARTITIONS: usize = 128;

/// How to split `bytes` of input when `available` bytes are free: the number of partitions, so one partition's rows and
/// working set fit (each targets a quarter of what is free; 2 to `MAX_PARTITIONS`), and each run file's write buffer,
/// so all buffers together take at most a quarter of what is free (4 KiB to 64 KiB).
pub(crate) fn partitions_for(bytes: u64, available: u64) -> (usize, usize) {
    let target = (available / 4).max(1 << 16);
    let parts = (bytes.div_ceil(target) as usize).clamp(2, MAX_PARTITIONS);
    let buffer = (available / (4 * parts as u64)).clamp(4 << 10, 64 << 10) as usize;
    (parts, buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_come_back_by_partition_in_input_order_and_the_directory_is_removed() {
        let scratch = std::env::temp_dir().join(format!("rdf-spill-test-{}", std::process::id()));
        let rows: Vec<Vec<TermId>> = (0..10_000u64).map(|i| vec![TermId(i % 37 + 1), TermId(i + 1)]).collect();
        let parts = Partitions::write(&scratch, 2, &[0], (8, 1 << 16), rows.iter().map(Vec::as_slice), || {}).unwrap();
        let dir = parts.dir.clone();
        assert!(dir.is_dir());
        let mut seen = vec![false; rows.len()];
        for p in 0..parts.count() {
            let (seqs, got) = parts.read(p).unwrap();
            assert!(seqs.windows(2).all(|w| w[0] < w[1]), "input order within a partition");
            for (s, r) in seqs.iter().zip(&got) {
                assert_eq!(&rows[*s as usize], r);
                seen[*s as usize] = true;
            }
            // one key lives in one partition
            let keys: std::collections::HashSet<_> = got.iter().map(|r| r[0]).collect();
            for q in (0..parts.count()).filter(|&q| q != p) {
                assert!(parts.read(q).unwrap().1.iter().all(|r| !keys.contains(&r[0])));
            }
        }
        assert!(seen.iter().all(|&s| s));
        drop(parts);
        assert!(!dir.exists());
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn an_unwind_while_writing_removes_the_run_files() {
        let scratch = std::env::temp_dir().join(format!("rdf-spill-unwind-{}", std::process::id()));
        let rows: Vec<Vec<TermId>> = (0..20_000u64).map(|i| vec![TermId(i + 1)]).collect();
        let mut calls = 0;
        let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Partitions::write(&scratch, 1, &[0], (4, 1 << 12), rows.iter().map(Vec::as_slice), || { calls += 1; if calls == 3 { panic!("stop") } })
        }));
        assert!(stopped.is_err());
        assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0, "no run directory is left behind");
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn a_sweep_removes_only_directories_nobody_holds() {
        let scratch = std::env::temp_dir().join(format!("rdf-spill-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        let rows: Vec<Vec<TermId>> = (0..100u64).map(|i| vec![TermId(i + 1)]).collect();
        let live = Partitions::write(&scratch, 1, &[0], (2, 1 << 12), rows.iter().map(Vec::as_slice), || {}).unwrap();
        // A killed process's directory: locked by nobody (this one was never locked), plus one with an unheld LOCK.
        let dead = scratch.join(format!("{PREFIX}999999-0"));
        std::fs::create_dir_all(&dead).unwrap();
        std::fs::write(dead.join("part-0"), b"x").unwrap();
        let unheld = scratch.join(format!("{PREFIX}999999-1"));
        std::fs::create_dir_all(&unheld).unwrap();
        std::fs::write(unheld.join("LOCK"), b"").unwrap();
        let other = scratch.join("unrelated");
        std::fs::create_dir_all(&other).unwrap();
        assert_eq!(sweep_stale(&scratch).unwrap(), 2);
        assert!(live.dir.is_dir(), "a directory whose owner is alive is kept");
        assert!(!dead.exists() && !unheld.exists());
        assert!(other.is_dir(), "only spill directories are touched");
        drop(live);
        assert_eq!(sweep_stale(&std::env::temp_dir().join("rdf-spill-sweep-missing")).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn partition_count_targets_a_quarter_of_what_is_free() {
        assert_eq!(partitions_for(100, 1 << 30), (2, 64 << 10));
        assert_eq!(partitions_for(1 << 30, 1 << 28), (16, 64 << 10));
        // 100 MB under a 3 MB budget: capped partitions, buffers within a quarter of what is free
        let (parts, buffer) = partitions_for(100 << 20, 3 << 20);
        assert_eq!(parts, MAX_PARTITIONS);
        assert!((parts * buffer) as u64 <= (3 << 20) / 4 + (parts * 4096) as u64);
        assert_eq!(partitions_for(u64::MAX, 1), (MAX_PARTITIONS, 4 << 10));
    }
}
