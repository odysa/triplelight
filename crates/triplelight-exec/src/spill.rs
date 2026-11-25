//! Spilling a blocking operator's input to disk under the memory budget (#111). Rows are written to run files under
//! `MemoryBudget::scratch_dir()`, hash-partitioned on the operator's key columns, each with its input position (`seq`),
//! so the operator can work one partition at a time and restore the order the in-memory path would produce.
//!
//! Each record is `seq` then the row's IDs, as little-endian `u64`s. The run files live in a fresh directory that
//! `Partitions` removes when it drops, which also happens when a cancelled or over-budget query unwinds.
use crate::model::TermId;
use std::fs::File;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// The rows of one operator's input, split by key hash into run files.
pub(crate) struct Partitions {
    dir: PathBuf,
    width: usize,
    rows: Vec<u64>,
    bytes: u64,
}

impl Partitions {
    /// Writes `rows` (each `width` IDs) into `parts` run files under a new directory in `scratch`, routed by a hash of
    /// the IDs at `key_cols`. `seq` is each row's position in `rows`.
    pub(crate) fn write<'r>(
        scratch: &Path,
        width: usize,
        key_cols: &[usize],
        parts: usize,
        rows: impl IntoIterator<Item = &'r [TermId]>,
        mut check: impl FnMut(),
    ) -> io::Result<Partitions> {
        let dir = fresh_dir(scratch)?;
        // From here on the directory is owned, so an error or unwind below still removes it.
        let mut out = Partitions { dir, width, rows: vec![0; parts], bytes: 0 };
        let mut files = Vec::with_capacity(parts);
        for p in 0..parts { files.push(BufWriter::with_capacity(1 << 16, File::create(out.path(p))?)); }
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
        for f in files { f.into_inner().map_err(|e| e.into_error())?.sync_data().ok(); }
        Ok(out)
    }
    pub(crate) fn count(&self) -> usize { self.rows.len() }
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

/// A directory of its own under `scratch`, unique within this process and across processes sharing `scratch`.
fn fresh_dir(scratch: &Path) -> io::Result<PathBuf> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::fs::create_dir_all(scratch)?;
    loop {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = scratch.join(format!(".rdf-spill-{}-{n}", std::process::id()));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

/// How many partitions to split `bytes` of input into so one partition's rows and its working set fit in `available`
/// bytes: each partition targets a quarter of what is free, between 2 and 1024 partitions.
pub(crate) fn partitions_for(bytes: u64, available: u64) -> usize {
    let target = (available / 4).max(1 << 16);
    (bytes.div_ceil(target) as usize).clamp(2, 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_come_back_by_partition_in_input_order_and_the_directory_is_removed() {
        let scratch = std::env::temp_dir().join(format!("rdf-spill-test-{}", std::process::id()));
        let rows: Vec<Vec<TermId>> = (0..10_000u64).map(|i| vec![TermId(i % 37 + 1), TermId(i + 1)]).collect();
        let parts = Partitions::write(&scratch, 2, &[0], 8, rows.iter().map(Vec::as_slice), || {}).unwrap();
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
            Partitions::write(&scratch, 1, &[0], 4, rows.iter().map(Vec::as_slice), || { calls += 1; if calls == 3 { panic!("stop") } })
        }));
        assert!(stopped.is_err());
        assert_eq!(std::fs::read_dir(&scratch).unwrap().count(), 0, "no run directory is left behind");
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn partition_count_targets_a_quarter_of_what_is_free() {
        assert_eq!(partitions_for(100, 1 << 30), 2);
        assert_eq!(partitions_for(1 << 30, 1 << 28), 16);
        assert_eq!(partitions_for(u64::MAX, 1), 1024);
    }
}
