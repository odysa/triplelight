//! Scratch-backed lexicographic byte-record sorter for bounded-memory ingestion.
//! This is a building block; it is not yet wired into snapshot creation.
use std::{
    cmp::Reverse,
    collections::BinaryHeap,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
const FAN_IN: usize = 32;
const BUF: usize = 8192;
static NONCE: AtomicU64 = AtomicU64::new(0);
fn bad(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

pub(super) struct ExternalSort {
    dir: PathBuf,
    pending: Vec<Vec<u8>>,
    used: usize,
    run_budget: usize,
    max_record: usize,
    /// Memory the merge heads may use (the `memory_bytes` given at construction).
    merge_budget: usize,
    /// Largest record pushed or imported so far; merge heads are sized from it, not from `max_record`.
    observed_max: usize,
    fan_in: usize,
    next: u64,
    runs: Vec<PathBuf>,
}
impl ExternalSort {
    /// `memory_bytes` bounds run payloads and merge heads (file buffers and
    /// allocator overhead are additional). Scratch is private and removed on drop.
    pub(super) fn new(parent: &Path, memory_bytes: usize, max_record: usize) -> io::Result<Self> {
        if max_record == 0 || max_record > u32::MAX as usize {
            return Err(bad("invalid maximum record size"));
        }
        let per_head = max_record
            .checked_add(BUF + 128)
            .ok_or_else(|| bad("sort budget overflow"))?;
        let fan_in = (memory_bytes / per_head).min(FAN_IN);
        if fan_in < 2 {
            return Err(bad("sort budget too small for merge"));
        }
        let dir = loop {
            let n = NONCE.fetch_add(1, Ordering::Relaxed);
            let d = parent.join(format!(".rdf-sort-{}-{n}", std::process::id()));
            match fs::create_dir(&d) {
                Ok(()) => break d,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        };
        Ok(Self {
            dir,
            pending: Vec::new(),
            used: 0,
            run_budget: memory_bytes,
            max_record,
            merge_budget: memory_bytes,
            observed_max: 0,
            fan_in,
            next: 0,
            runs: Vec::new(),
        })
    }
    /// Private scratch directory shared with the consumer of the sorted stream.
    pub(super) fn scratch_dir(&self) -> &Path {
        &self.dir
    }

    fn path(&mut self) -> PathBuf {
        let n = self.next;
        self.next += 1;
        self.dir.join(format!("run-{n}"))
    }
    pub(super) fn push(&mut self, record: Vec<u8>) -> io::Result<()> {
        if record.len() > self.max_record {
            return Err(bad("sort record exceeds configured maximum"));
        }
        let cost = record
            .capacity()
            .checked_add(std::mem::size_of::<Vec<u8>>())
            .ok_or_else(|| bad("record size overflow"))?;
        if cost > self.run_budget {
            return Err(bad("sort record exceeds run budget"));
        }
        if !self.pending.is_empty() && self.used.saturating_add(cost) > self.run_budget {
            self.flush()?;
        }
        self.observed_max = self.observed_max.max(record.len());
        self.used += cost;
        self.pending.push(record);
        Ok(())
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        self.pending.sort_unstable();
        let path = self.path();
        let mut out = BufWriter::with_capacity(
            BUF,
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?,
        );
        for record in &self.pending {
            write_record(&mut out, record)?;
        }
        out.flush()?;
        drop(out);
        self.pending.clear();
        self.used = 0;
        self.runs.push(path);
        Ok(())
    }
    /// Adopt sorted files from other private sorters on the same filesystem.
    /// Each successful rename transfers cleanup ownership to this sorter.
    /// `observed_max` is the largest record in those files (their sorter's `observed_max()`).
    pub(super) fn import_sorted(&mut self, paths: &[PathBuf], observed_max: usize) -> io::Result<()> {
        if !self.pending.is_empty() || !self.runs.is_empty() { return Err(bad("import requires an empty sorter")); }
        for source in paths {
            let path = self.path();
            fs::rename(source, &path)?;
            self.runs.push(path);
        }
        self.observed_max = self.observed_max.max(observed_max);
        Ok(())
    }
    pub(super) fn observed_max(&self) -> usize {
        self.observed_max
    }
    /// Merge width: heads are sized for the largest record actually seen, never narrower than the construction-time
    /// width (which assumed `max_record`).
    fn merge_width(&self) -> usize {
        let per_head = self.observed_max.saturating_add(BUF + 128).max(1);
        (self.merge_budget / per_head).min(FAN_IN).max(self.fan_in)
    }
    /// Returns a private scratch file containing sorted length-prefixed records.
    /// It remains valid until this sorter is dropped.
    /// Merge rounds until at most `limit` runs remain (`limit` of at least 1).
    fn reduce_runs(&mut self, limit: usize) -> io::Result<()> {
        let width = self.merge_width();
        while self.runs.len() > limit.max(1) {
            let old = std::mem::take(&mut self.runs);
            let mut next = Vec::new();
            for group in old.chunks(width) {
                let path = self.path();
                if let [only] = group {
                    // A lone run is already sorted; move it instead of copying it through a merge.
                    fs::rename(only, &path)?;
                } else {
                    merge(group, &path, self.max_record)?;
                    // Free this group's inputs now, not after the whole round, so a round does not hold both its
                    // inputs and its outputs. A failure leaves files only in the private scratch directory, which
                    // `Drop` removes.
                    for input in group {
                        fs::remove_file(input)?;
                    }
                }
                next.push(path);
            }
            self.runs = next;
        }
        Ok(())
    }
    pub(super) fn finish(&mut self) -> io::Result<PathBuf> {
        self.flush()?;
        if self.runs.is_empty() {
            let path = self.path();
            File::create(&path)?;
            self.runs.push(path);
        }
        self.reduce_runs(1)?;
        Ok(self.runs[0].clone())
    }
    /// Like `finish`, but the last merge is not written out: the returned stream yields the sorted records straight
    /// from at most one merge width of runs. The stream borrows the sorter, so the scratch directory outlives it.
    pub(super) fn finish_stream(&mut self) -> io::Result<SortedStream<'_>> {
        self.flush()?;
        let width = self.merge_width();
        self.reduce_runs(width)?;
        let max = self.max_record;
        let mut readers = self
            .runs
            .iter()
            .map(|p| File::open(p).map(|f| BufReader::with_capacity(BUF, f)))
            .collect::<io::Result<Vec<_>>>()?;
        let mut heap = BinaryHeap::new();
        for (i, r) in readers.iter_mut().enumerate() {
            if let Some(record) = read_record(r, max)? {
                heap.push(Reverse((record, i)));
            }
        }
        Ok(SortedStream { sort: self, readers, heap })
    }
}
/// Sorted records read lazily from the final runs. Records equal as bytes are indistinguishable, so the order is the
/// same as that of the file `finish` would write.
pub(super) struct SortedStream<'a> {
    sort: &'a mut ExternalSort,
    readers: Vec<BufReader<File>>,
    heap: BinaryHeap<Reverse<(Vec<u8>, usize)>>,
}
impl SortedStream<'_> {
    pub(super) fn next_record(&mut self) -> io::Result<Option<Vec<u8>>> {
        let Some(Reverse((record, i))) = self.heap.pop() else { return Ok(None) };
        if let Some(next) = read_record(&mut self.readers[i], self.sort.max_record)? {
            self.heap.push(Reverse((next, i)));
        }
        Ok(Some(record))
    }
    /// Remove the run files once the stream has been fully consumed.
    pub(super) fn discard(self) -> io::Result<()> {
        let SortedStream { sort, readers, .. } = self;
        drop(readers);
        for path in std::mem::take(&mut sort.runs) {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}
impl Drop for ExternalSort {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn write_record(out: &mut impl Write, record: &[u8]) -> io::Result<()> {
    let n = u32::try_from(record.len()).map_err(|_| bad("record too large"))?;
    out.write_all(&n.to_le_bytes())?;
    out.write_all(record)
}
pub(super) fn read_record(input: &mut impl Read, max: usize) -> io::Result<Option<Vec<u8>>> {
    let mut head = [0u8; 4];
    loop {
        match input.read(&mut head[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    input.read_exact(&mut head[1..])?;
    let len = u32::from_le_bytes(head) as usize;
    if len > max {
        return Err(bad("corrupt sort record length"));
    }
    let mut bytes = vec![0; len];
    input.read_exact(&mut bytes)?;
    Ok(Some(bytes))
}
fn merge(paths: &[PathBuf], dest: &Path, max: usize) -> io::Result<()> {
    let mut readers = paths
        .iter()
        .map(|p| File::open(p).map(|f| BufReader::with_capacity(BUF, f)))
        .collect::<io::Result<Vec<_>>>()?;
    let mut heap = BinaryHeap::new();
    for (i, r) in readers.iter_mut().enumerate() {
        if let Some(record) = read_record(r, max)? {
            heap.push(Reverse((record, i)));
        }
    }
    let mut out = BufWriter::with_capacity(
        BUF,
        OpenOptions::new().write(true).create_new(true).open(dest)?,
    );
    while let Some(Reverse((record, i))) = heap.pop() {
        write_record(&mut out, &record)?;
        if let Some(next) = read_record(&mut readers[i], max)? {
            heap.push(Reverse((next, i)));
        }
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multiple_passes_keep_duplicates_and_clean_scratch() {
        let parent = std::env::temp_dir();
        let scratch;
        {
            let mut sort = ExternalSort::new(&parent, 20_000, 1000).unwrap();
            // Force many small runs and more than one merge pass.
            sort.run_budget = 128;
            let mut expected = Vec::new();
            for i in (0..1000u32).rev() {
                let record = (i % 311).to_be_bytes().to_vec();
                sort.push(record.clone()).unwrap();
                expected.push(record);
            }
            let path = sort.finish().unwrap();
            scratch = sort.dir.clone();
            let mut reader = BufReader::new(File::open(path).unwrap());
            let mut actual = Vec::new();
            while let Some(r) = read_record(&mut reader, 1000).unwrap() {
                actual.push(r);
            }
            expected.sort_unstable();
            assert_eq!(actual, expected);
        }
        assert!(!scratch.exists());
    }
    #[test]
    fn each_merge_group_frees_its_inputs_and_a_lone_run_is_moved() {
        let parent = std::env::temp_dir();
        let mut sort = ExternalSort::new(&parent, 60_000, 1000).unwrap();
        sort.run_budget = 128;
        sort.fan_in = 3;
        // 10 small runs: groups of 3, 3, 3 and a lone run, then another round.
        let mut expected = Vec::new();
        for i in (0..400u32).rev() {
            let record = (i % 97).to_be_bytes().to_vec();
            sort.push(record.clone()).unwrap();
            expected.push(record);
        }
        let path = sort.finish().unwrap();
        // Only the final merged run remains in the scratch directory.
        let files = fs::read_dir(&sort.dir).unwrap().count();
        assert_eq!(files, 1, "intermediate runs must be gone after finish");
        let mut reader = BufReader::new(File::open(path).unwrap());
        let mut actual = Vec::new();
        while let Some(r) = read_record(&mut reader, 1000).unwrap() {
            actual.push(r);
        }
        expected.sort_unstable();
        assert_eq!(actual, expected);
    }
    fn drain(stream: &mut SortedStream<'_>) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while let Some(r) = stream.next_record().unwrap() {
            out.push(r);
        }
        out
    }
    fn push_all(sort: &mut ExternalSort, count: u32) -> Vec<Vec<u8>> {
        let mut expected = Vec::new();
        for i in (0..count).rev() {
            let record = (i % 53).to_be_bytes().to_vec();
            sort.push(record.clone()).unwrap();
            expected.push(record);
        }
        expected.sort_unstable();
        expected
    }

    #[test]
    fn stream_yields_the_same_records_as_finish() {
        for count in [0u32, 1, 40, 700] {
            let parent = std::env::temp_dir();
            let mut a = ExternalSort::new(&parent, 60_000, 1000).unwrap();
            let mut b = ExternalSort::new(&parent, 60_000, 1000).unwrap();
            a.run_budget = 96;
            b.run_budget = 96;
            a.fan_in = 3;
            b.fan_in = 3;
            a.merge_budget = 0; // keep the narrow width so more than one round is needed
            b.merge_budget = 0;
            let expected = push_all(&mut a, count);
            push_all(&mut b, count);
            let path = a.finish().unwrap();
            let mut reader = BufReader::new(File::open(path).unwrap());
            let mut from_file = Vec::new();
            while let Some(r) = read_record(&mut reader, 1000).unwrap() {
                from_file.push(r);
            }
            let mut stream = b.finish_stream().unwrap();
            let streamed = drain(&mut stream);
            assert_eq!(streamed, from_file, "count {count}");
            assert_eq!(streamed, expected, "count {count}");
            let scratch = stream.sort.dir.clone();
            stream.discard().unwrap();
            assert_eq!(fs::read_dir(&scratch).unwrap().count(), 0, "runs are removed after discard");
            drop(b);
            assert!(!scratch.exists());
        }
    }

    #[test]
    fn merge_width_follows_the_largest_record_seen() {
        let parent = std::env::temp_dir();
        let mut sort = ExternalSort::new(&parent, 128 * 1024 * 1024, 16 * 1024 * 1024 + 10).unwrap();
        assert_eq!(sort.fan_in, 7, "construction assumes the worst-case record");
        sort.push(vec![1; 100]).unwrap();
        assert_eq!(sort.merge_width(), FAN_IN, "small records allow the full width");
        sort.push(vec![2; 15 * 1024 * 1024]).unwrap();
        assert_eq!(sort.merge_width(), 8, "a 15 MiB record needs wider heads");
        // Imported runs carry the largest record their sorter saw.
        let mut other = ExternalSort::new(&parent, 128 * 1024 * 1024, 16 * 1024 * 1024 + 10).unwrap();
        other.import_sorted(&[], 14 * 1024 * 1024).unwrap();
        assert_eq!(other.observed_max(), 14 * 1024 * 1024);
    }

    #[test]
    fn truncated_and_oversized_records_fail() {
        assert!(read_record(&mut &[1u8, 0][..], 10).is_err());
        assert!(read_record(&mut &[20u8, 0, 0, 0][..], 10).is_err());
        assert!(read_record(&mut &[3u8, 0, 0, 0, 1][..], 10).is_err());
        assert!(ExternalSort::new(&std::env::temp_dir(), 100, 1000).is_err());
    }
}
