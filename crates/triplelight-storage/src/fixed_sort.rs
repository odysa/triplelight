//! External sort of fixed-width byte records (task #90), for the disk build's order sorts and its term-ID mapping.
//!
//! The same contract as `ExternalSort` for records of exactly `W` bytes: the output is the records in lexicographic
//! byte order, duplicates kept. Runs hold raw records with no length prefix, a run fills with `memory / W` records (no
//! per-record allocation or header), and the in-run sort compares 8-byte big-endian chunks, which is byte order. The
//! merge is a heap over at most `FAN_IN` runs, as in `ExternalSort`.
use std::{
    cmp::{Ordering, Reverse},
    collections::BinaryHeap,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering as AtomicOrdering},
};
const FAN_IN: usize = 32;
const BUF: usize = 64 * 1024;
static NONCE: AtomicU64 = AtomicU64::new(0);

/// Lexicographic byte order of two records, compared eight bytes at a time.
fn compare<const W: usize>(a: &[u8; W], b: &[u8; W]) -> Ordering {
    let mut i = 0;
    while i + 8 <= W {
        let x = u64::from_be_bytes(a[i..i + 8].try_into().unwrap());
        let y = u64::from_be_bytes(b[i..i + 8].try_into().unwrap());
        if x != y { return x.cmp(&y) }
        i += 8;
    }
    a[i..].cmp(&b[i..])
}

/// Reads one record. `Ok(false)` at a clean end of input; a partial record is an error.
pub(super) fn read_fixed<const W: usize>(input: &mut impl Read, record: &mut [u8; W]) -> io::Result<bool> {
    let mut filled = 0;
    while filled < W {
        match input.read(&mut record[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "truncated sort record")),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

pub(super) struct FixedSort<const W: usize> {
    dir: PathBuf,
    pending: Vec<[u8; W]>,
    /// Records per run: the memory budget divided by the record width.
    capacity: usize,
    fan_in: usize,
    next: u64,
    runs: Vec<PathBuf>,
}
impl<const W: usize> FixedSort<W> {
    /// `memory_bytes` bounds a run's records and the merge's read buffers (allocator overhead is additional). Scratch
    /// is a private directory under `parent`, removed on drop.
    pub(super) fn new(parent: &Path, memory_bytes: usize) -> io::Result<Self> {
        let capacity = memory_bytes / W;
        let fan_in = (memory_bytes / (BUF + W)).min(FAN_IN);
        if W == 0 || capacity == 0 || fan_in < 2 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "sort budget too small"));
        }
        let dir = loop {
            let n = NONCE.fetch_add(1, AtomicOrdering::Relaxed);
            let d = parent.join(format!(".rdf-fixed-sort-{}-{n}", std::process::id()));
            match fs::create_dir(&d) {
                Ok(()) => break d,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        };
        Ok(Self { dir, pending: Vec::new(), capacity, fan_in, next: 0, runs: Vec::new() })
    }
    fn path(&mut self) -> PathBuf {
        let n = self.next;
        self.next += 1;
        self.dir.join(format!("run-{n}"))
    }
    pub(super) fn push(&mut self, record: [u8; W]) -> io::Result<()> {
        if self.pending.len() == self.capacity { self.flush()? }
        if self.pending.capacity() == 0 { self.pending.reserve_exact(self.capacity) }
        self.pending.push(record);
        Ok(())
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.pending.is_empty() { return Ok(()) }
        self.pending.sort_unstable_by(compare::<W>);
        let path = self.path();
        let mut out = BufWriter::with_capacity(BUF, OpenOptions::new().write(true).create_new(true).open(&path)?);
        for record in &self.pending { out.write_all(record)? }
        out.flush()?;
        drop(out);
        self.pending.clear();
        self.runs.push(path);
        Ok(())
    }
    /// Sorts everything pushed and returns a private scratch file of the records in order, valid until this sorter is
    /// dropped.
    pub(super) fn finish(&mut self) -> io::Result<PathBuf> {
        self.flush()?;
        if self.runs.is_empty() {
            let path = self.path();
            File::create(&path)?;
            self.runs.push(path);
        }
        while self.runs.len() > 1 {
            let old = std::mem::take(&mut self.runs);
            let mut next = Vec::new();
            for group in old.chunks(self.fan_in) {
                let path = self.path();
                if let [only] = group {
                    fs::rename(only, &path)?;
                } else {
                    merge::<W>(group, &path)?;
                    // Free this group's inputs now, so a round never holds both all its inputs and all its outputs.
                    for input in group { fs::remove_file(input)? }
                }
                next.push(path);
            }
            self.runs = next;
        }
        Ok(self.runs[0].clone())
    }
}
impl<const W: usize> Drop for FixedSort<W> {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn merge<const W: usize>(paths: &[PathBuf], dest: &Path) -> io::Result<()> {
    let mut readers = paths
        .iter()
        .map(|p| File::open(p).map(|f| BufReader::with_capacity(BUF, f)))
        .collect::<io::Result<Vec<_>>>()?;
    // `[u8; W]` orders lexicographically, the same order as `compare`.
    let mut heap = BinaryHeap::with_capacity(readers.len());
    let mut record = [0u8; W];
    for (i, r) in readers.iter_mut().enumerate() {
        if read_fixed(r, &mut record)? { heap.push(Reverse((record, i))) }
    }
    let mut out = BufWriter::with_capacity(BUF, OpenOptions::new().write(true).create_new(true).open(dest)?);
    while let Some(Reverse((head, i))) = heap.pop() {
        out.write_all(&head)?;
        if read_fixed(&mut readers[i], &mut record)? { heap.push(Reverse((record, i))) }
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::external_sort::{ExternalSort, read_record};

    /// `run` records per run (to force many runs and merge rounds in a small test), at a 1 MiB budget otherwise.
    fn sorted_by_fixed<const W: usize>(records: &[[u8; W]], run: Option<usize>) -> Vec<[u8; W]> {
        let mut sort = FixedSort::<W>::new(&std::env::temp_dir(), 1 << 20).unwrap();
        if let Some(run) = run { sort.capacity = run }
        for r in records { sort.push(*r).unwrap() }
        let path = sort.finish().unwrap();
        let mut input = BufReader::new(File::open(path).unwrap());
        let (mut out, mut r) = (Vec::new(), [0u8; W]);
        while read_fixed(&mut input, &mut r).unwrap() { out.push(r) }
        let dir = sort.dir.clone();
        drop(sort);
        assert!(!dir.exists(), "scratch is removed on drop");
        out
    }
    fn sorted_by_external<const W: usize>(records: &[[u8; W]]) -> Vec<[u8; W]> {
        let mut sort = ExternalSort::new(&std::env::temp_dir(), 1 << 20, W).unwrap();
        for r in records { sort.push(r.to_vec()).unwrap() }
        let path = sort.finish().unwrap();
        let mut input = BufReader::new(File::open(path).unwrap());
        let mut out = Vec::new();
        while let Some(r) = read_record(&mut input, W).unwrap() { out.push(r.try_into().unwrap()) }
        out
    }
    fn records<const W: usize>(n: usize, seed: u64, distinct: u64) -> Vec<[u8; W]> {
        let mut x = seed | 1;
        (0..n).map(|_| {
            let mut r = [0u8; W];
            for chunk in r.chunks_mut(8) {
                x ^= x << 13; x ^= x >> 7; x ^= x << 17;
                let v = (x % distinct).to_be_bytes();
                chunk.copy_from_slice(&v[8 - chunk.len()..]);
            }
            r
        }).collect()
    }

    /// The fixed-width sorter returns exactly what the byte-record sorter does: random, duplicate-heavy and
    /// constant inputs, an empty input, a width that is not a multiple of eight (the 17-byte mapping), and budgets
    /// small enough to force many runs and more than one merge round (32-way fan-in).
    #[test]
    fn matches_the_byte_record_sorter() {
        for (n, distinct) in [(0, 1), (1, 1), (5_000, u64::MAX), (5_000, 7), (5_000, 1)] {
            let rows = records::<24>(n, 0x9e37_79b9_7f4a_7c15 ^ n as u64, distinct);
            let expect = sorted_by_external(&rows);
            for run in [None, Some(1), Some(50), Some(4_999)] {
                assert_eq!(sorted_by_fixed(&rows, run), expect, "n={n} distinct={distinct} run={run:?}");
            }
            let wide = records::<17>(n, 0x2545_f491_4f6c_dd1d ^ n as u64, distinct);
            assert_eq!(sorted_by_fixed(&wide, Some(40)), sorted_by_external(&wide), "17-byte n={n}");
        }
        // 5,000 runs of 8 against the 15-way fan-in of a 1 MiB budget: four merge rounds, some with a lone run renamed.
        let rows = records::<24>(40_000, 7, u64::MAX);
        assert_eq!(sorted_by_fixed(&rows, Some(8)), sorted_by_external(&rows));
    }
    #[test]
    fn rejects_a_budget_too_small_and_a_truncated_record() {
        assert!(FixedSort::<24>::new(&std::env::temp_dir(), 1024).is_err());
        let mut short: &[u8] = &[1, 2, 3];
        assert!(read_fixed::<24>(&mut short, &mut [0; 24]).is_err());
        let mut empty: &[u8] = &[];
        assert!(!read_fixed::<24>(&mut empty, &mut [0; 24]).unwrap());
    }
}
