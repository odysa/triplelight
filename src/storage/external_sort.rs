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
            fan_in,
            next: 0,
            runs: Vec::new(),
        })
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
    /// Returns a private scratch file containing sorted length-prefixed records.
    /// It remains valid until this sorter is dropped.
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
                merge(group, &path, self.max_record)?;
                next.push(path);
                // Runs are private scratch. Once their replacement is complete,
                // discard this group before starting the next to reduce peak space.
                for input in group {
                    fs::remove_file(input)?;
                }
            }
            self.runs = next;
        }
        Ok(self.runs[0].clone())
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
    fn failed_later_group_cleans_private_scratch_on_drop() {
        let parent = std::env::temp_dir();
        let scratch;
        {
            let mut sort = ExternalSort::new(&parent, 20_000, 1000).unwrap();
            sort.run_budget = 40;
            sort.fan_in = 2;
            for i in 0..20u32 { sort.push(i.to_be_bytes().to_vec()).unwrap(); }
            sort.flush().unwrap();
            assert!(sort.runs.len() >= 4);
            let old = sort.runs.clone();
            fs::remove_file(&old[2]).unwrap();
            scratch = sort.dir.clone();
            assert!(sort.finish().is_err());
            assert!(!old[0].exists() && !old[1].exists());
            assert!(scratch.exists());
        }
        assert!(!scratch.exists());
    }
    #[test]
    fn truncated_and_oversized_records_fail() {
        assert!(read_record(&mut &[1u8, 0][..], 10).is_err());
        assert!(read_record(&mut &[20u8, 0, 0, 0][..], 10).is_err());
        assert!(read_record(&mut &[3u8, 0, 0, 0, 1][..], 10).is_err());
        assert!(ExternalSort::new(&std::env::temp_dir(), 100, 1000).is_err());
    }
}
