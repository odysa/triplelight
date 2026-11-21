//! External construction of compressed snapshots; bounded run buffers, with scratch on disk.
use super::{
    Snapshot,
    disk_index::{DiskIndex, write_index_stream},
    disk_snapshot::{CATALOG_FILE, FILES, QUAD_FILES},
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
/// The longest N-Triples term text a base can hold (task #45: the WAL refuses longer terms at commit).
pub(crate) const MAX_TERM: usize = triplelight_core::ntriples::MAX_TOKEN;
const _: () = assert!(MAX_TERM == 16 * 1024 * 1024, "the base limit is oxttl's 16 MiB token limit");
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
    // Triple rows are (s, p, o); the key is the order's permutation of them.
    let order = perm.order();
    build_index::<3>(dir, raw, triples, name, run_budget, &|row: &[u8], key: &mut Vec<u8>| {
        for i in order { key.extend_from_slice(&row[i * 8..i * 8 + 8]) }
    })
}
/// One graph-first quad order (task #36): raw rows are (g, s, p, o) and the key is g followed by the triple order.
fn build_quad_permutation(dir: &Path, raw: &Path, quads: u64, name: &str, perm: Perm, run_budget: usize) -> io::Result<[u8; 32]> {
    let order = perm.order();
    build_index::<4>(dir, raw, quads, name, run_budget, &|row: &[u8], key: &mut Vec<u8>| {
        key.extend_from_slice(&row[..8]);
        for i in order { key.extend_from_slice(&row[8 + i * 8..16 + i * 8]) }
    })
}
/// Sorts `count` raw rows of `N` big-endian IDs by `key_of`, drops duplicates and writes them as index `name`.
fn build_index<const N: usize>(dir: &Path, raw: &Path, count: u64, name: &str, run_budget: usize, key_of: &(dyn Fn(&[u8], &mut Vec<u8>) + Sync)) -> io::Result<[u8; 32]>
where
    super::disk_index::Width<N>: super::disk_index::SupportedWidth<N>,
{
    let width = N * 8;
    let mut sorter = ExternalSort::new(dir, run_budget, width)?;
    let mut input = BufReader::new(File::open(raw)?);
    let mut row = vec![0u8; width];
    for _ in 0..count {
        input.read_exact(&mut row)?;
        let mut key = Vec::with_capacity(width);
        key_of(&row, &mut key);
        sorter.push(key)?;
    }
    let sorted = sorter.finish()?;
    let mut reader = UniqueRows::<N>::open(&sorted)?;
    let mut unique = 0u64;
    while reader.next().transpose()?.is_some() { unique += 1; }
    let rows = UniqueRows::<N>::open(&sorted)?;
    let path = dir.join(name);
    let mut output = BufWriter::new(OpenOptions::new().write(true).create_new(true).open(&path)?);
    write_index_stream(&mut output, unique, rows)?;
    output.flush()?; output.get_ref().sync_all()?; drop(output);
    Ok(DiskIndex::<N>::open(path)?.identity())
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
    /// Named-graph rows (task #36), added after every triple: quads (s, p, o, g), then the name of every graph in
    /// the catalog (so empty graphs get an ID too). Row IDs run triples, then quads, then graph names.
    quads: u64,
    graph_names: u64,
    index_workers: usize,
}
impl Builder {
    fn new(dir: &Path) -> io::Result<Self> { Self::with_budget(dir, MEMORY) }
    fn with_budget(dir: &Path, bytes: usize) -> io::Result<Self> {
        Ok(Self {
            terms: ExternalSort::new(dir, bytes, MAX_TERM + 10)?,
            triples: 0,
            quads: 0,
            graph_names: 0,
            index_workers: 1,
        })
    }
    fn add(&mut self, terms: [String; 3]) -> io::Result<()> {
        self.add_with_id(terms, self.triples)
    }
    fn add_with_id(&mut self, terms: [String; 3], triple_id: u64) -> io::Result<()> {
        if self.quads + self.graph_names != 0 {
            return Err(bad("triples must be added before named-graph rows"));
        }
        self.push_terms(terms, triple_id)?;
        self.triples = self
            .triples
            .checked_add(1)
            .ok_or_else(|| bad("too many triples"))?;
        Ok(())
    }
    /// A triple of named graph `g`, as the terms `[s, p, o, g]`.
    fn add_quad(&mut self, terms: [String; 4]) -> io::Result<()> {
        if self.graph_names != 0 {
            return Err(bad("quads must be added before graph names"));
        }
        self.push_terms(terms, self.triples + self.quads)?;
        self.quads = self.quads.checked_add(1).ok_or_else(|| bad("too many quads"))?;
        Ok(())
    }
    /// A graph of the catalog (each once), whether or not it has triples.
    fn add_graph_name(&mut self, name: String) -> io::Result<()> {
        self.push_terms([name], self.triples + self.quads + self.graph_names)?;
        self.graph_names = self.graph_names.checked_add(1).ok_or_else(|| bad("too many graphs"))?;
        Ok(())
    }
    fn push_terms<const K: usize>(&mut self, terms: [String; K], row_id: u64) -> io::Result<()> {
        let triple_id = row_id;
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
        Ok(())
    }
    fn finish(mut self, dir: &Path) -> io::Result<()> {
        // The last merge is not written out: records stream straight from the runs into the dictionary pass.
        let mut stream = self.terms.finish_stream()?;
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
        while let Some(r) = stream.next_record()? {
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
        // The term runs have been fully consumed; free them before the mapping sort and index builds.
        stream.discard()?;
        checkpoint(1)?;
        let assignments = mapping.finish()?;
        let mut input = BufReader::new(File::open(&assignments)?);
        let raw = dir.join(".triples.raw");
        let quad_raw = dir.join(".quads.raw");
        let mut triples =
            BufWriter::new(OpenOptions::new().write(true).create_new(true).open(&raw)?);
        // Created whenever there are named graphs, also when all of them are empty (the quad orders are then built
        // from an empty file).
        let mut quads = if self.quads + self.graph_names > 0 {
            Some(BufWriter::new(OpenOptions::new().write(true).create_new(true).open(&quad_raw)?))
        } else {
            None
        };
        let mut graph_ids: Vec<u64> = Vec::new();
        let mut previous_id = None;
        let rows = self.triples + self.quads + self.graph_names;
        for row in 0..rows {
            let arity = if row < self.triples { 3 } else if row < self.triples + self.quads { 4 } else { 1 };
            let mut current_id = None;
            let mut ids = [[0u8; 8]; 4];
            for pos in 0..arity as u8 {
                let r =
                    read_record(&mut input, 17)?.ok_or_else(|| bad("missing term assignment"))?;
                if r.len() != 17 || r[8] != pos { return Err(bad("invalid term assignment")); }
                let id = u64::from_be_bytes(r[..8].try_into().unwrap());
                if pos == 0 {
                    if previous_id.is_some_and(|prev| id <= prev) { return Err(bad("unordered term assignment")); }
                    current_id = Some(id);
                } else if current_id != Some(id) { return Err(bad("invalid term assignment")); }
                ids[pos as usize].copy_from_slice(&r[9..17]);
            }
            previous_id = current_id;
            match arity {
                3 => for id in &ids[..3] { triples.write_all(id)? },
                // Quad rows are stored graph first: (g, s, p, o).
                4 => {
                    let out = quads.as_mut().expect("quad rows exist");
                    out.write_all(&ids[3])?;
                    for id in &ids[..3] { out.write_all(id)? }
                }
                _ => graph_ids.push(u64::from_be_bytes(ids[0])),
            }
        }
        if let Some(mut q) = quads.take() { q.flush()?; }
        if read_record(&mut input, 17)?.is_some() {
            return Err(bad("extra term assignment"));
        }
        triples.flush()?;
        drop(triples);
        drop(input);
        fs::remove_file(&assignments)?;
        checkpoint(2)?;
        let dictionary_identity = super::lazy_dictionary::convert_unified(dir)?;
        checkpoint(3)?;
        let mut identities = vec![dictionary_identity];
        let jobs: Vec<_> = FILES[1..].iter().copied().zip(Perm::ALL).collect();
        let workers = self.index_workers;
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
        // Named graphs: six graph-first quad orders and the catalog, only when the dataset has named graphs, so a
        // default-only base stays byte-identical to an RDFSNAP4 base.
        let named = self.quads + self.graph_names > 0;
        if named {
            let quad_jobs: Vec<_> = QUAD_FILES.iter().copied().zip(Perm::ALL).collect();
            for batch in quad_jobs.chunks(workers) {
                let results = std::thread::scope(|scope| {
                    let handles: Vec<_> = batch.iter().map(|&(name, perm)| {
                        let raw = &quad_raw; let quads = self.quads;
                        scope.spawn(move || build_quad_permutation(dir, raw, quads, name, perm, run_budget))
                    }).collect();
                    handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(io::Error::other("permutation build worker panicked")))).collect::<Vec<_>>()
                });
                for result in results { identities.push(result?); }
            }
            fs::remove_file(&quad_raw)?;
            identities.push(write_catalog(dir, graph_ids)?);
        }
        // Finish and remove all scratch before publishing the manifest.
        drop(mapping);
        drop(self.terms);
        let mut manifest = BufWriter::new(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dir.join("MANIFEST"))?,
        );
        // RDFSNAP5 (task #36) = RDFSNAP4 plus six quad orders and the graph catalog.
        manifest.write_all(if named { b"RDFSNAP5" } else { b"RDFSNAP4" })?;
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
/// Writes the graph catalog: every named graph's ID (ascending, each once) with its triple count from the G+SPO
/// order, empty graphs included. Layout: magic `RDFGRA01`, count, then `(id, triples)` pairs (little-endian), then
/// the SHA-256 of everything before it, which is also the file's identity in the manifest.
fn write_catalog(dir: &Path, mut graph_ids: Vec<u64>) -> io::Result<[u8; 32]> {
    use sha2::{Digest, Sha256};
    let gspo = DiskIndex::<4>::open(dir.join(QUAD_FILES[0]))?;
    graph_ids.sort_unstable();
    let before = graph_ids.len();
    graph_ids.dedup();
    if graph_ids.len() != before || graph_ids.contains(&0) {
        return Err(bad("graph catalog names a graph twice"));
    }
    let mut body = Vec::with_capacity(16 + graph_ids.len() * 16);
    body.extend_from_slice(b"RDFGRA01");
    body.extend_from_slice(&(graph_ids.len() as u64).to_le_bytes());
    let mut total = 0u64;
    for &g in &graph_ids {
        let range = gspo.range(&[TermId(g)])?;
        let n = range.end - range.start;
        total += n;
        body.extend_from_slice(&g.to_le_bytes());
        body.extend_from_slice(&n.to_le_bytes());
    }
    if total != gspo.len() {
        return Err(bad("a quad names a graph outside the catalog"));
    }
    let identity: [u8; 32] = Sha256::digest(&body).into();
    let path = dir.join(CATALOG_FILE);
    let mut out = BufWriter::new(OpenOptions::new().write(true).create_new(true).open(&path)?);
    out.write_all(&body)?;
    out.write_all(&identity)?;
    out.flush()?;
    out.get_ref().sync_all()?;
    Ok(identity)
}
struct UniqueRows<const N: usize> {
    input: BufReader<File>,
    previous: Option<Vec<u8>>,
    done: bool,
}
impl<const N: usize> UniqueRows<N> {
    fn open(path: &Path) -> io::Result<Self> {
        Ok(Self {
            input: BufReader::new(File::open(path)?),
            previous: None,
            done: false,
        })
    }
}
impl<const N: usize> Iterator for UniqueRows<N> {
    type Item = io::Result<[TermId; N]>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        loop {
            match read_record(&mut self.input, N * 8) {
                Ok(Some(bytes)) => {
                    if bytes.len() != N * 8 {
                        self.done = true;
                        return Some(Err(bad("bad triple run")));
                    }
                    if self.previous.as_deref() == Some(&bytes[..]) {
                        continue;
                    }
                    let row = std::array::from_fn(|i| {
                        TermId(u64::from_be_bytes(
                            bytes[i * 8..i * 8 + 8].try_into().unwrap(),
                        ))
                    });
                    self.previous = Some(bytes);
                    return Some(Ok(row));
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
                for t in triplelight_core::ntriples::read_ntriples(input) {
                    let t: Triple = t?;
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
    let mut observed = 0usize;
    let mut count = 0u64;
    for (b, path) in &builders {
        count = count.checked_add(b.triples).ok_or_else(|| bad("too many triples"))?;
        paths.push(path.clone());
        observed = observed.max(b.terms.observed_max());
    }
    let mut combined = Builder::new(dir)?;
    combined.terms.import_sorted(&paths, observed)?;
    combined.triples = count;
    combined.index_workers = shards.min(3);
    drop(builders);
    combined.finish(dir)
}

pub(super) fn from_nt(dir: &Path, path: Option<&Path>) -> io::Result<()> {
    fs::create_dir(dir)?;
    let mut builder = Builder::new(dir)?;
    if let Some(path) = path {
        let input = BufReader::new(File::open(path)?);
        for t in triplelight_core::ntriples::read_ntriples(input) {
            let t: Triple = t?;
            builder.add([
                t.subject.to_string(),
                t.predicate.to_string(),
                t.object.to_string(),
            ])?;
        }
    }
    builder.finish(dir)
}
/// A base of the default graph and every named graph of `source` (task #36): `RDFSNAP5` if it has named graphs,
/// otherwise exactly `from_snapshot` of its default graph.
pub(super) fn from_dataset(dir: &Path, source: &dyn triplelight_core::storage::DatasetView) -> io::Result<()> {
    let names = source.graph_names()?;
    if names.is_empty() { return from_snapshot(dir, source.default_graph()) }
    fs::create_dir(dir)?;
    let mut builder = Builder::new(dir)?;
    let text = |view: &dyn Snapshot, id: TermId| -> io::Result<String> {
        let mut out = String::new();
        view.with_term(id, &mut |v| out = v.to_string())?;
        Ok(out)
    };
    let default = source.default_graph();
    for batch in default.scan(Perm::Spo, &[]) {
        let batch = batch?;
        for i in 0..batch.len() {
            builder.add([text(default, batch.cols[0][i])?, text(default, batch.cols[1][i])?, text(default, batch.cols[2][i])?])?;
        }
    }
    for &g in &names {
        let view = source.graph(g)?.ok_or_else(|| bad("graph listed but absent"))?;
        let name = text(default, g)?;
        for batch in view.scan(Perm::Spo, &[]) {
            let batch = batch?;
            for i in 0..batch.len() {
                builder.add_quad([text(view, batch.cols[0][i])?, text(view, batch.cols[1][i])?, text(view, batch.cols[2][i])?, name.clone()])?;
            }
        }
    }
    for &g in &names { builder.add_graph_name(text(default, g)?)?; }
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
        for name in FILES[1..].iter().copied().chain(["dictionary.unified"]) { assert_eq!(fs::read(a.join(name)).unwrap(), fs::read(b.join(name)).unwrap(), "{name}"); }
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
        assert_eq!(names, ["MANIFEST", "dictionary.unified", "ops", "osp", "pos", "pso", "sop", "spo"]);
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
    fn snapshot_format_fence_refuses_mismatched_manifests() {
        let dir = std::env::temp_dir().join(format!("rdf-ext-fence-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        let input = small_input(&dir);
        let out = dir.join("base");
        from_nt(&out, Some(&input)).unwrap();
        let manifest = fs::read(out.join("MANIFEST")).unwrap();
        assert_eq!(&manifest[..8], b"RDFSNAP4");
        assert_eq!(manifest.len(), 8 + 7 * 32, "one dictionary identity and six indexes");
        DiskSnapshot::open(&out).unwrap().verify().unwrap();
        // A reader that only knows RDFSNAP3 would expect eight identities and two dictionary files; the current code
        // path for 3 must refuse this store rather than misread it.
        let mut as_v3 = manifest.clone();
        as_v3[..8].copy_from_slice(b"RDFSNAP3");
        fs::write(out.join("MANIFEST"), &as_v3).unwrap();
        assert!(DiskSnapshot::open(&out).is_err(), "a v4 store read as v3 must be refused");
        // Unknown future versions are refused.
        let mut future = manifest.clone();
        future[..8].copy_from_slice(b"RDFSNAP9");
        fs::write(out.join("MANIFEST"), &future).unwrap();
        assert!(DiskSnapshot::open(&out).is_err());
        // Restoring the manifest makes it open again.
        fs::write(out.join("MANIFEST"), &manifest).unwrap();
        DiskSnapshot::open(&out).unwrap();
        fs::remove_dir_all(dir).unwrap();
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
