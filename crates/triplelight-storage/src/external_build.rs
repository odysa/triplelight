//! External construction of compressed snapshots; bounded run buffers, with scratch on disk.
use super::{
    Snapshot,
    disk_index::{DiskIndex, write_index_stream},
    disk_snapshot::FILES,
    external_sort::{ExternalSort, read_record},
};
use crate::model::{Perm, TermId};
use oxrdf::{Term, Triple};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
};
const MEMORY: usize = 128 * 1024 * 1024;
const MAX_TERM: usize = 16 * 1024 * 1024;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
static BUILD_BUDGET_MB: AtomicUsize = AtomicUsize::new(0);
static FORCE_EXTERNAL: AtomicBool = AtomicBool::new(false);
/// Sets an advisory strategy budget, not a hard process RSS limit.
pub fn set_build_budget_mb(mb: usize) -> io::Result<()> {
    if mb == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "build budget must be nonzero",
        ));
    }
    BUILD_BUDGET_MB.store(mb, Ordering::Relaxed);
    Ok(())
}
pub fn force_external_build() {
    FORCE_EXTERNAL.store(true, Ordering::Relaxed);
}
/// Permutation sorts that may run at once: each holds one run buffer plus merge heads and read buffers, so allow one
/// per `2 * MEMORY` of the advisory budget, never more than the CPUs available or the six permutations, and at least one.
fn index_workers_for(budget_mb: usize, cpus: usize) -> usize {
    (budget_mb.saturating_mul(1_048_576) / (2 * MEMORY)).min(cpus).clamp(1, Perm::ALL.len())
}
fn index_workers() -> usize {
    index_workers_for(budget_mb(), std::thread::available_parallelism().map_or(1, |n| n.get()))
}
fn budget_mb() -> usize {
    let n = BUILD_BUDGET_MB.load(Ordering::Relaxed);
    if n != 0 {
        return n;
    }
    super::disk_index::physical_memory_bytes().map_or(256, |n| usize::try_from(n / 2 / 1_048_576).unwrap_or(256).max(1))
}
pub(super) fn adaptive(dir: &Path, source: Option<&Path>, workers: usize) -> io::Result<()> {
    if let Some(path) = source {
        let bytes = fs::metadata(path)?.len();
        let budget = (budget_mb() as u128) * 1_048_576;
        if !FORCE_EXTERNAL.load(Ordering::Relaxed) && (bytes as u128) * 3 <= budget {
            eprintln!("disk build strategy: in-memory (estimated 3x input; not an RSS limit)");
            let snapshot =
                super::MemorySnapshot::from_ntriples_parallel_file(path, workers.max(1))?;
            return super::DiskSnapshot::create(dir, &snapshot);
        }
    }
    eprintln!("disk build strategy: external sort (budget is advisory, not an RSS limit)");
    from_nt_parallel(dir, source, workers.max(1))
}
fn build_permutation(dir: &Path, raw: &Path, triples: u64, name: &str, perm: Perm, run_budget: usize) -> io::Result<[u8; 32]> {
    let mut sorter = ExternalSort::new(dir, run_budget, 24)?;
    let mut input = BufReader::new(File::open(raw)?);
    for _ in 0..triples {
        let mut triple = [0u8; 24];
        input.read_exact(&mut triple)?;
        let mut key = Vec::with_capacity(24);
        for i in perm.order() { key.extend_from_slice(&triple[i * 8..i * 8 + 8]); }
        sorter.push(key)?;
    }
    let sorted = sorter.finish()?;
    let mut reader = UniqueRows::open(&sorted)?;
    let mut count = 0u64;
    while reader.next().transpose()?.is_some() { count += 1; }
    let rows = UniqueRows::open(&sorted)?;
    let path = dir.join(name);
    let mut output = BufWriter::new(OpenOptions::new().write(true).create_new(true).open(&path)?);
    write_index_stream(&mut output, count, rows)?;
    output.flush()?; output.get_ref().sync_all()?; drop(output);
    Ok(DiskIndex::open(path)?.identity())
}
// Test-only failure injection: `checkpoint(n)` fails when the test selected phase `n`. Compiled out in normal builds.
#[cfg(test)]
thread_local! { static FAIL_AT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
fn checkpoint(n: usize) -> io::Result<()> {
    if FAIL_AT.with(|f| f.get()) == n { Err(io::Error::other("injected failure")) } else { Ok(()) }
}
#[cfg(not(test))]
#[inline(always)]
fn checkpoint(_n: usize) -> io::Result<()> { Ok(()) }

fn bad(s: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s)
}

pub(super) struct Builder {
    terms: ExternalSort,
    triples: u64,
    index_workers: usize,
}
impl Builder {
    fn new(dir: &Path) -> io::Result<Self> { Self::with_budget(dir, MEMORY) }
    fn with_budget(dir: &Path, bytes: usize) -> io::Result<Self> {
        Ok(Self {
            terms: ExternalSort::new(dir, bytes, MAX_TERM + 10)?,
            triples: 0,
            index_workers: index_workers(),
        })
    }
    fn add(&mut self, terms: [String; 3]) -> io::Result<()> {
        self.add_with_id(terms, self.triples)
    }
    fn add_with_id(&mut self, terms: [String; 3], triple_id: u64) -> io::Result<()> {
        for (pos, term) in terms.into_iter().enumerate() {
            if term.len() > MAX_TERM || term.as_bytes().contains(&0) {
                return Err(bad("invalid or oversized encoded RDF term"));
            }
            let mut r = term.into_bytes();
            r.push(0);
            r.extend_from_slice(&triple_id.to_be_bytes());
            r.push(pos as u8);
            self.terms.push(r)?;
        }
        self.triples = self
            .triples
            .checked_add(1)
            .ok_or_else(|| bad("too many triples"))?;
        Ok(())
    }
    fn finish(mut self, dir: &Path) -> io::Result<()> {
        let sorted = self.terms.finish()?;
        let mut input = BufReader::new(File::open(&sorted)?);
        let mut mapping = ExternalSort::new(dir, MEMORY, 17)?;
        let mut dict = BufWriter::new(
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(dir.join(FILES[0]))?,
        );
        dict.write_all(b"RDFDIC01")?;
        dict.write_all(&0u64.to_le_bytes())?;
        let mut previous = Vec::new();
        let mut current = TermId(0);
        let mut unique = 0u64;
        while let Some(r) = read_record(&mut input, MAX_TERM + 10)? {
            if r.len() < 10 || r[r.len() - 10] != 0 {
                return Err(bad("malformed term run"));
            }
            let text = &r[..r.len() - 10];
            if current.0 == 0 || previous != text {
                let term: Term = std::str::from_utf8(text)
                    .map_err(|_| bad("invalid UTF-8 term"))?
                    .parse()
                    .map_err(|_| bad("invalid RDF term"))?;
                if unique >= (1u64 << 60) {
                    return Err(bad("too many RDF terms"));
                }
                let kind = match term {
                    Term::NamedNode(_) => 1u64,
                    Term::BlankNode(_) => 2,
                    Term::Literal(_) => 3,
                };
                current = TermId((kind << 60) | unique);
                unique += 1;
                dict.write_all(&current.0.to_le_bytes())?;
                dict.write_all(&(text.len() as u64).to_le_bytes())?;
                dict.write_all(text)?;
                previous.clear();
                previous.extend_from_slice(text);
            }
            let tail = &r[r.len() - 9..];
            let mut entry = Vec::with_capacity(17);
            entry.extend_from_slice(tail);
            entry.extend_from_slice(&current.0.to_be_bytes());
            mapping.push(entry)?;
        }
        dict.flush()?;
        dict.seek(SeekFrom::Start(8))?;
        dict.write_all(&unique.to_le_bytes())?;
        dict.flush()?;
        dict.get_ref().sync_all()?;
        drop(dict);
        drop(input);
        // The merged term file has been fully consumed; free it before the mapping sort and index builds.
        fs::remove_file(&sorted)?;
        checkpoint(1)?;
        let assignments = mapping.finish()?;
        let mut input = BufReader::new(File::open(&assignments)?);
        let raw = dir.join(".triples.raw");
        let mut triples =
            BufWriter::new(OpenOptions::new().write(true).create_new(true).open(&raw)?);
        let mut previous_id = None;
        for _ in 0..self.triples {
            let mut current_id = None;
            for pos in 0..3u8 {
                let r =
                    read_record(&mut input, 17)?.ok_or_else(|| bad("missing term assignment"))?;
                if r.len() != 17 || r[8] != pos { return Err(bad("invalid term assignment")); }
                let id = u64::from_be_bytes(r[..8].try_into().unwrap());
                if pos == 0 {
                    if previous_id.is_some_and(|prev| id <= prev) { return Err(bad("unordered term assignment")); }
                    current_id = Some(id);
                } else if current_id != Some(id) { return Err(bad("invalid term assignment")); }
                triples.write_all(&r[9..17])?;
            }
            previous_id = current_id;
        }
        if read_record(&mut input, 17)?.is_some() {
            return Err(bad("extra term assignment"));
        }
        triples.flush()?;
        drop(triples);
        drop(input);
        fs::remove_file(&assignments)?;
        checkpoint(2)?;
        let (term_identity, id_identity) = super::lazy_dictionary::convert_legacy(dir, true)?;
        checkpoint(3)?;
        let mut identities = vec![term_identity, id_identity];
        let jobs: Vec<_> = FILES[1..].iter().copied().zip(Perm::ALL).collect();
        let workers = self.index_workers;
        eprintln!("permutation sorts: {workers} at a time");
        let run_budget = (budget_mb().saturating_mul(1_048_576) / workers).min(MEMORY);
        for batch in jobs.chunks(workers) {
            let results = std::thread::scope(|scope| {
                let handles: Vec<_> = batch.iter().map(|&(name, perm)| {
                    let raw = &raw; let triples = self.triples;
                    scope.spawn(move || build_permutation(dir, raw, triples, name, perm, run_budget))
                }).collect();
                handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(io::Error::other("permutation build worker panicked")))).collect::<Vec<_>>()
            });
            // Join all workers before returning any failure, so no background writer survives.
            for result in results { identities.push(result?); }
        }
        fs::remove_file(raw)?;
        // Finish and remove all scratch before publishing the manifest.
        drop(mapping);
        drop(self.terms);
        let mut manifest = BufWriter::new(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dir.join("MANIFEST"))?,
        );
        manifest.write_all(b"RDFSNAP3")?;
        for identity in identities {
            manifest.write_all(&identity)?;
        }
        manifest.flush()?;
        manifest.get_ref().sync_all()?;
        drop(manifest);
        File::open(dir)?.sync_all()?;
        File::open(
            dir.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?
        .sync_all()?;
        Ok(())
    }
}
struct UniqueRows {
    input: BufReader<File>,
    previous: Option<[u8; 24]>,
    done: bool,
}
impl UniqueRows {
    fn open(path: &Path) -> io::Result<Self> {
        Ok(Self {
            input: BufReader::new(File::open(path)?),
            previous: None,
            done: false,
        })
    }
}
impl Iterator for UniqueRows {
    type Item = io::Result<[TermId; 3]>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        loop {
            match read_record(&mut self.input, 24) {
                Ok(Some(r)) => {
                    let bytes: [u8; 24] = match r.try_into() {
                        Ok(v) => v,
                        Err(_) => {
                            self.done = true;
                            return Some(Err(bad("bad triple run")));
                        }
                    };
                    if self.previous == Some(bytes) {
                        continue;
                    }
                    self.previous = Some(bytes);
                    return Some(Ok(std::array::from_fn(|i| {
                        TermId(u64::from_be_bytes(
                            bytes[i * 8..i * 8 + 8].try_into().unwrap(),
                        ))
                    })));
                }
                Ok(None) => {
                    self.done = true;
                    return None;
                }
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            }
        }
    }
}
/// Find the next line boundary without buffering an arbitrarily long line.
fn next_line_boundary(path: &Path, offset: u64, size: u64) -> io::Result<u64> {
    if offset == 0 || offset >= size { return Ok(offset.min(size)); }
    let mut file = BufReader::new(File::open(path)?);
    file.seek(SeekFrom::Start(offset - 1))?;
    let mut prev = [0u8; 1]; file.read_exact(&mut prev)?;
    if prev[0] == b'\n' { return Ok(offset); }
    let mut at = offset;
    loop {
        let buf = file.fill_buf()?;
        if buf.is_empty() { return Ok(size); }
        let n = buf.iter().position(|&b| b == b'\n').map_or(buf.len(), |i| i + 1);
        let found = buf[n - 1] == b'\n';
        file.consume(n); at += n as u64;
        if found { return Ok(at); }
    }
}

/// Eight independent parsers create private sorted runs; worker IDs occupy disjoint ranges.
/// The split is only used for N-Triples (Turtle grammar is not line splittable).
fn from_nt_parallel(dir: &Path, path: Option<&Path>, workers: usize) -> io::Result<()> {
    let Some(path) = path else { return from_nt(dir, None); };
    // A worker's merge must hold at least two maximum-length records and file buffers.
    // The global limit is an advisory allocation target, not an RSS guarantee.
    let budget = budget_mb().saturating_mul(1_048_576);
    let minimum = 2 * (MAX_TERM + 10 + 8192 + 128);
    let shards = workers.clamp(1, 8).min((budget / minimum).max(1));
    if shards == 1 { return from_nt(dir, Some(path)); }
    let worker_budget = (budget / shards).min(MEMORY);
    let size = fs::metadata(path)?.len();
    let mut boundaries = vec![0u64];
    for i in 1..shards {
        boundaries.push(next_line_boundary(path, (size as u128 * i as u128 / shards as u128) as u64, size)?);
    }
    boundaries.push(size);
    fs::create_dir(dir)?;
    let results = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for (shard, limits) in boundaries.windows(2).enumerate() {
            let (start, end) = (limits[0], limits[1]);
            handles.push(scope.spawn(move || -> io::Result<(Builder, std::path::PathBuf)> {
                let mut b = Builder::with_budget(dir, worker_budget)?;
                let mut file = File::open(path)?; file.seek(SeekFrom::Start(start))?;
                let input = BufReader::new(file.take(end - start));
                for t in oxttl::NTriplesParser::new().for_reader(input) {
                    let t: Triple = t.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    if b.triples >= (1u64 << 56) { return Err(bad("too many triples in shard")); }
                    let id = ((shard as u64) << 56) | b.triples;
                    b.add_with_id([t.subject.to_string(), t.predicate.to_string(), t.object.to_string()], id)?;
                }
                let sorted = b.terms.finish()?;
                Ok((b, sorted))
            }));
        }
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(io::Error::other("N-Triples worker panicked")))).collect::<Vec<_>>()
    });
    let builders = results.into_iter().collect::<io::Result<Vec<_>>>()?;
    let mut paths = Vec::with_capacity(builders.len());
    let mut count = 0u64;
    for (b, path) in &builders {
        count = count.checked_add(b.triples).ok_or_else(|| bad("too many triples"))?;
        paths.push(path.clone());
    }
    let mut combined = Builder::new(dir)?;
    combined.terms.import_sorted(&paths)?;
    combined.triples = count;
    drop(builders);
    combined.finish(dir)
}

pub(super) fn from_nt(dir: &Path, path: Option<&Path>) -> io::Result<()> {
    fs::create_dir(dir)?;
    let mut builder = Builder::new(dir)?;
    if let Some(path) = path {
        let input = BufReader::new(File::open(path)?);
        for t in oxttl::NTriplesParser::new().for_reader(input) {
            let t: Triple = t.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            builder.add([
                t.subject.to_string(),
                t.predicate.to_string(),
                t.object.to_string(),
            ])?;
        }
    }
    builder.finish(dir)
}
pub(super) fn from_snapshot(dir: &Path, source: &dyn Snapshot) -> io::Result<()> {
    fs::create_dir(dir)?;
    let mut builder = Builder::new(dir)?;
    for batch in source.scan(Perm::Spo, &[]) {
        let batch = batch?;
        if batch.cols.iter().any(|c| c.len() != batch.len()) {
            return Err(bad("inconsistent source batch"));
        }
        #[allow(clippy::needless_range_loop)] // columnar batch: index all three columns by row
        for i in 0..batch.len() {
            let mut terms = [String::new(), String::new(), String::new()];
            for (c, term) in terms.iter_mut().enumerate() {
                source.with_term(batch.cols[c][i], &mut |v| *term = v.to_string())?;
            }
            builder.add(terms)?;
        }
    }
    builder.finish(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        exec::{QueryResult, query},
        storage::DiskSnapshot,
    };
    #[test]
    fn sharded_nt_preserves_terms_across_boundaries() {
        let root = std::env::temp_dir().join(format!("rdf-shards-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root); fs::create_dir(&root).unwrap();
        let file = root.join("input.nt");
        fs::write(&file, "# comment\r\n<urn:s> <urn:p> \"é\"@fr .\r\n\n_:a <urn:p> <urn:o> .\n<urn:s> <urn:p> \"é\"@fr .\n<urn:last> <urn:p> \"x\" .").unwrap();
        let a = root.join("serial"); let b = root.join("parallel");
        from_nt(&a, Some(&file)).unwrap(); from_nt_parallel(&b, Some(&file), 8).unwrap();
        for name in FILES[1..].iter().copied().chain(["dictionary.id", "dictionary.term"]) { assert_eq!(fs::read(a.join(name)).unwrap(), fs::read(b.join(name)).unwrap(), "{name}"); }
        assert_eq!(fs::read(a.join("MANIFEST")).unwrap(), fs::read(b.join("MANIFEST")).unwrap());
        let snap = DiskSnapshot::open(&b).unwrap(); snap.verify().unwrap(); drop(snap);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn parallel_index_failure_does_not_publish_manifest() {
        let root = std::env::temp_dir().join(format!("rdf-index-failure-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root); fs::create_dir(&root).unwrap();
        let mut builder = Builder::new(&root).unwrap();
        builder.index_workers = 3;
        builder.add(["<urn:s>".into(), "<urn:p>".into(), "<urn:o>".into()]).unwrap();
        fs::write(root.join("spo"), b"collision").unwrap();
        assert!(builder.finish(&root).is_err());
        assert!(!root.join("MANIFEST").exists());
        assert!(!fs::read_dir(&root).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with(".rdf-sort-")));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn sharded_blank_node_labels_are_document_scoped() {
        let root = std::env::temp_dir().join(format!("rdf-shards-bnode-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root); fs::create_dir(&root).unwrap();
        let input = root.join("input.nt");
        let mut data = String::from("_:x <urn:p> <urn:one> .\n");
        for i in 0..1000 { data.push_str(&format!("<urn:s{i}> <urn:p> <urn:o> .\n")); }
        data.push_str("_:x <urn:q> <urn:two> .\n");
        fs::write(&input, data).unwrap();
        let out = root.join("base"); from_nt_parallel(&out, Some(&input), 8).unwrap();
        let snap = DiskSnapshot::open(&out).unwrap();
        let result = query(&snap, "ASK { ?x <urn:p> <urn:one>; <urn:q> <urn:two> }").unwrap();
        assert!(matches!(result, QueryResult::Boolean(true)));
        drop(snap); fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn sharded_nt_rejects_invalid_and_empty_works() {
        let root = std::env::temp_dir().join(format!("rdf-shards-invalid-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root); fs::create_dir(&root).unwrap();
        let file = root.join("input.nt");
        fs::write(&file, "<urn:a> <urn:p> <urn:o> .\nnot a triple\n").unwrap();
        let invalid = root.join("invalid");
        assert!(from_nt_parallel(&invalid, Some(&file), 8).is_err());
        assert!(!invalid.join("MANIFEST").exists());
        fs::write(&file, "").unwrap();
        let empty = root.join("empty"); from_nt_parallel(&empty, Some(&file), 8).unwrap();
        let snap = DiskSnapshot::open(&empty).unwrap(); snap.verify().unwrap(); drop(snap);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn ntriples_deduplicate_and_preserve_rdf_terms() {
        let dir = std::env::temp_dir().join(format!("rdf-ext-test-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let input = dir.join("input.nt");
        let output = dir.join("base");
        fs::write(&input,"<urn:s> <urn:p> \"hello\"@en .\n<urn:s> <urn:p> \"hello\"@en .\n_:x <urn:p> \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n").unwrap();
        from_nt(&output, Some(&input)).unwrap();
        let snapshot = DiskSnapshot::open(&output).unwrap();
        snapshot.verify().unwrap();
        let result = query(&snapshot, "SELECT ?s ?o WHERE { ?s <urn:p> ?o }").unwrap();
        match result {
            QueryResult::Solutions { rows, .. } => assert_eq!(rows.len(), 2),
            _ => panic!("unexpected result"),
        }
        drop(snapshot);
        fs::remove_dir_all(dir).unwrap();
    }
    fn small_input(dir: &Path) -> std::path::PathBuf {
        let input = dir.join("input.nt");
        let mut text = String::new();
        for i in 0..300 { text += &format!("<urn:s{i}> <urn:p{}> \"v{i}\" .\n<urn:s{i}> <urn:q> _:b{} .\n", i % 7, i % 11); }
        fs::write(&input, text).unwrap();
        input
    }

    #[test]
    fn early_scratch_deletion_leaves_only_the_published_files() {
        let dir = std::env::temp_dir().join(format!("rdf-ext-clean-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let input = small_input(&dir);
        let out = dir.join("base");
        from_nt(&out, Some(&input)).unwrap();
        let mut names: Vec<_> = fs::read_dir(&out).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        assert_eq!(names, ["MANIFEST", "dictionary.id", "dictionary.term", "ops", "osp", "pos", "pso", "sop", "spo"]);
        DiskSnapshot::open(&out).unwrap().verify().unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failure_after_each_early_deletion_publishes_no_manifest() {
        for phase in 1..=3 {
            let dir = std::env::temp_dir().join(format!("rdf-ext-fail{phase}-{}", std::process::id()));
            fs::create_dir(&dir).unwrap();
            let input = small_input(&dir);
            let out = dir.join("base");
            FAIL_AT.with(|f| f.set(phase));
            let result = from_nt(&out, Some(&input));
            FAIL_AT.with(|f| f.set(0));
            assert!(result.is_err(), "phase {phase} should fail");
            assert!(!out.join("MANIFEST").exists(), "phase {phase} must not publish a manifest");
            assert!(DiskSnapshot::open(&out).is_err(), "phase {phase} must not open as a snapshot");
            fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn index_workers_follow_the_advisory_budget_and_the_cpus() {
        assert_eq!(index_workers_for(0, 16), 1);
        assert_eq!(index_workers_for(255, 16), 1);
        assert_eq!(index_workers_for(256, 16), 1);
        assert_eq!(index_workers_for(768, 16), 3);
        assert_eq!(index_workers_for(1024, 16), 4);
        assert_eq!(index_workers_for(1536, 16), 6);
        assert_eq!(index_workers_for(1 << 20, 16), 6);
        // Never more sorts than CPUs, and never fewer than one.
        assert_eq!(index_workers_for(1 << 20, 4), 4);
        assert_eq!(index_workers_for(1 << 20, 1), 1);
        assert_eq!(index_workers_for(1536, 2), 2);
        assert_eq!(index_workers_for(1536, 0), 1);
    }

    #[test]
    fn invalid_input_does_not_publish_a_manifest() {
        let parent = std::env::temp_dir().join(format!("rdf-ext-fail-{}", std::process::id()));
        fs::create_dir(&parent).unwrap();
        let input = parent.join("bad.nt");
        fs::write(&input, "<invalid\n").unwrap();
        let out = parent.join("base");
        assert!(from_nt(&out, Some(&input)).is_err());
        assert!(!out.join("MANIFEST").exists());
        fs::remove_dir_all(parent).unwrap();
    }
}
