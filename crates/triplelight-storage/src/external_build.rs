//! External construction of compressed snapshots; bounded run buffers, with scratch on disk.
use super::{
    Snapshot,
    disk_index::{DiskIndex, write_index_stream},
    base_format::{CATALOG_FILE, FILES, QUAD_FILES},
    external_sort::ExternalSort,
    fixed_sort::{FixedSort, read_fixed},
};
use crate::model::{Perm, TermId};
use triplelight_rdf::{Term, Triple};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
};
const MEMORY: usize = 128 * 1024 * 1024;
use super::base_format::MAX_TERM;
/// Row ID and position of a term record that belongs to no row (an RDF 1.2 triple-term part, `Builder::push_parts`).
/// Real row IDs are far smaller (at most `8 << 56` in a sharded build), so these assignments sort after every row.
#[cfg_attr(not(feature = "rdf-12"), allow(dead_code))]
const FREE_ROW: u64 = u64::MAX;
#[cfg_attr(not(feature = "rdf-12"), allow(dead_code))]
const FREE_POS: u8 = 0xff;
const _: () = assert!(MAX_TERM == 16 * 1024 * 1024, "the base limit is oxttl's 16 MiB token limit");
/// Longest record of the term sort: the term text plus its flag and ID bytes.
const TERM_SORT_RECORD_MAX: usize = MAX_TERM + 10;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
static BUILD_BUDGET_MB: AtomicUsize = AtomicUsize::new(0);
static FORCE_EXTERNAL: AtomicBool = AtomicBool::new(false);
static KEPT_ORDERS: AtomicU8 = AtomicU8::new(super::base_format::REQUIRED_ORDERS);
#[cfg(test)]
thread_local! { static TEST_ORDERS: std::cell::Cell<Option<u8>> = const { std::cell::Cell::new(None) }; }
/// Tests run in parallel, so a test that needs another order set than the default sets it for its own thread.
#[cfg(test)]
pub(crate) fn keep_orders_on_this_thread(orders: u8) { TEST_ORDERS.with(|c| c.set(Some(orders))) }
fn kept_orders_setting() -> u8 {
    #[cfg(test)]
    if let Some(orders) = TEST_ORDERS.with(|c| c.get()) { return orders }
    KEPT_ORDERS.load(Ordering::Relaxed)
}
/// Which orders this process's new bases (imports and `build-disk`) keep, as bits of `Perm::ALL` positions (task
/// #73). The default, `REQUIRED_ORDERS` (SPO, PSO, POS, OSP), writes an `RDFSNAP6` base; `ALL_ORDERS` writes the
/// six-order `RDFSNAP4`/`RDFSNAP5` layout byte for byte, which older binaries can open. Compaction keeps the order set
/// of the base it replaces instead. The set must contain `REQUIRED_ORDERS`.
pub fn set_kept_orders(orders: u8) -> io::Result<()> {
    use super::base_format::{ALL_ORDERS, REQUIRED_ORDERS};
    if orders & !ALL_ORDERS != 0 || orders & REQUIRED_ORDERS != REQUIRED_ORDERS {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "an order set must keep SPO, PSO, POS and OSP"));
    }
    KEPT_ORDERS.store(orders, Ordering::Relaxed);
    Ok(())
}
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
/// Whether `adaptive` (in `disk_snapshot`) may build `path` in memory: six orders are kept, the external path is not
/// forced, and three times the input fits the build budget. The decision itself is the caller's; this crate's external
/// writer only reports whether its budget allows the in-memory route.
pub(super) fn in_memory_fits(path: &Path) -> io::Result<bool> {
    let bytes = fs::metadata(path)?.len();
    let budget = (budget_mb() as u128) * 1_048_576;
    // The in-memory path writes all six orders (RDFSNAP3), so any smaller order set takes the external builder.
    let six = kept_orders_setting() == super::base_format::ALL_ORDERS;
    Ok(six && !FORCE_EXTERNAL.load(Ordering::Relaxed) && (bytes as u128) * 3 <= budget)
}
fn build_permutation(dir: &Path, raw: &Path, triples: u64, name: &str, perm: Perm, run_budget: usize) -> io::Result<[u8; 32]> {
    // Triple rows are (s, p, o); the key is the order's permutation of them.
    let order = perm.order();
    build_index::<3, 24>(dir, raw, triples, name, run_budget, &|row: &[u8], key: &mut [u8; 24]| {
        for (k, i) in order.into_iter().enumerate() { key[k * 8..k * 8 + 8].copy_from_slice(&row[i * 8..i * 8 + 8]) }
    })
}
/// One graph-first quad order (task #36): raw rows are (g, s, p, o) and the key is g followed by the triple order.
fn build_quad_permutation(dir: &Path, raw: &Path, quads: u64, name: &str, perm: Perm, run_budget: usize) -> io::Result<[u8; 32]> {
    let order = perm.order();
    build_index::<4, 32>(dir, raw, quads, name, run_budget, &|row: &[u8], key: &mut [u8; 32]| {
        key[..8].copy_from_slice(&row[..8]);
        for (k, i) in order.into_iter().enumerate() { key[8 + k * 8..16 + k * 8].copy_from_slice(&row[8 + i * 8..16 + i * 8]) }
    })
}
/// Sorts `count` raw rows of `N` big-endian IDs (`W` = `8 * N` bytes) by `key_of`, drops duplicates and writes them
/// as index `name`.
fn build_index<const N: usize, const W: usize>(dir: &Path, raw: &Path, count: u64, name: &str, run_budget: usize, key_of: &(dyn Fn(&[u8], &mut [u8; W]) + Sync)) -> io::Result<[u8; 32]>
where
    super::disk_index::Width<N>: super::disk_index::SupportedWidth<N>,
{
    const { assert!(W == 8 * N) };
    let mut sorter = FixedSort::<W>::new(dir, run_budget)?;
    let mut input = BufReader::new(File::open(raw)?);
    let mut row = [0u8; W];
    let mut key = [0u8; W];
    for _ in 0..count {
        input.read_exact(&mut row)?;
        key_of(&row, &mut key);
        sorter.push(key)?;
    }
    let sorted = sorter.finish()?;
    let mut reader = UniqueRows::<N, W>::open(&sorted)?;
    let mut unique = 0u64;
    while reader.next().transpose()?.is_some() { unique += 1; }
    let rows = UniqueRows::<N, W>::open(&sorted)?;
    let path = dir.join(name);
    let mut output = BufWriter::new(OpenOptions::new().write(true).create_new(true).open(&path)?);
    write_index_stream(&mut output, unique, rows)?;
    output.flush()?; output.get_ref().sync_all()?; drop(output);
    Ok(DiskIndex::<N>::open(path)?.identity())
}
// Test-only failure injection: `checkpoint(n)` fails when the test selected phase `n`. Compiled out in normal builds.
#[cfg(test)]
thread_local! { static FAIL_AT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
/// Tests: make the build fail at phase `n` on this thread (0: never).
#[cfg(test)]
pub(super) fn fail_at_on_this_thread(n: usize) { FAIL_AT.with(|f| f.set(n)) }
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
    /// The orders `finish` writes (bits of `Perm::ALL` positions), from `KEPT_ORDERS` at construction.
    orders: u8,
    /// Deduplicating parse (task #91): per shard, the file of triples as local term IDs, its count of local IDs and its
    /// triple count. When set, the term sort holds each shard's distinct terms, not every occurrence, and `finish` maps
    /// the rows to global IDs by array lookup instead of sorting one record per occurrence.
    dedup_rows: Option<Vec<DedupShard>>,
}
/// One shard of a deduplicating parse: its rows file (three big-endian local IDs per triple), the number of local IDs it
/// handed out (dense from 0) and its number of triples.
struct DedupShard { rows: std::path::PathBuf, locals: u64, triples: u64 }
impl Builder {
    fn new(dir: &Path) -> io::Result<Self> { Self::with_budget(dir, MEMORY) }
    fn with_budget(dir: &Path, bytes: usize) -> io::Result<Self> {
        Ok(Self {
            terms: ExternalSort::new(dir, bytes, TERM_SORT_RECORD_MAX)?,
            triples: 0,
            quads: 0,
            graph_names: 0,
            index_workers: 1,
            orders: kept_orders_setting(),
            dedup_rows: None,
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
            #[cfg(feature = "rdf-12")]
            if term.starts_with("<<(") { self.push_parts(&term)? }
            let mut r = term.into_bytes();
            r.push(0);
            r.extend_from_slice(&triple_id.to_be_bytes());
            r.push(pos as u8);
            self.terms.push(r)?;
        }
        Ok(())
    }
    /// RDF 1.2 (task #101): every part of triple term `text`, nested ones included, becomes a dictionary term even
    /// if it occurs nowhere else, so `Dictionary::triple_term_parts` resolves on a disk base. Parts are pushed as
    /// row-less records (`FREE_ROW`, `FREE_POS`), which get IDs like any term and are skipped when rows are built.
    #[cfg(feature = "rdf-12")]
    fn push_parts(&mut self, text: &str) -> io::Result<()> {
        let Term::Triple(t) = text.parse::<Term>().map_err(|_| bad("invalid RDF term"))? else { return Ok(()) };
        let t = *t;
        for part in [t.subject.to_string(), t.predicate.to_string(), t.object.to_string()] {
            if part.len() > MAX_TERM { return Err(bad("invalid or oversized encoded RDF term")) }
            if part.starts_with("<<(") { self.push_parts(&part)? }
            let mut r = part.into_bytes();
            r.push(0);
            r.extend_from_slice(&FREE_ROW.to_be_bytes());
            r.push(FREE_POS);
            self.terms.push(r)?;
        }
        Ok(())
    }
    fn finish(mut self, dir: &Path) -> io::Result<()> {
        // The last merge is not written out: records stream straight from the runs into the dictionary pass.
        let mut stream = self.terms.finish_stream()?;
        let mut mapping = FixedSort::<17>::new(dir, MEMORY)?;
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
        // Whether any RDF 1.2 term (triple term or directional literal) is in the dictionary: the base is then
        // written as RDFSNAP7, which carries `has_rdf12_terms` and which builds without RDF 1.2 support refuse.
        #[cfg_attr(not(feature = "rdf-12"), allow(unused_mut))]
        let mut rdf12 = false;
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
                #[cfg(feature = "rdf-12")]
                if matches!(&term, Term::Triple(_)) || matches!(&term, Term::Literal(l) if l.direction().is_some()) { rdf12 = true }
                let kind = match term {
                    Term::NamedNode(_) => crate::model::id_tag::IRI,
                    Term::BlankNode(_) => crate::model::id_tag::BLANK_NODE,
                    Term::Literal(_) => crate::model::id_tag::LITERAL,
                    #[cfg(feature = "rdf-12")]
                    Term::Triple(_) => crate::model::id_tag::TRIPLE_TERM,
                };
                current = TermId((kind << 60) | unique);
                unique += 1;
                dict.write_all(&current.0.to_le_bytes())?;
                dict.write_all(&(text.len() as u64).to_le_bytes())?;
                dict.write_all(text)?;
                previous.clear();
                previous.extend_from_slice(text);
            }
            // (row ID, position, term ID): sorted by row ID, then position.
            let mut entry = [0u8; 17];
            entry[..9].copy_from_slice(&r[r.len() - 9..]);
            entry[9..].copy_from_slice(&current.0.to_be_bytes());
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
        if let Some(shards) = self.dedup_rows.take() {
            // The deduplicating parse reads N-Triples only; it has no quad or graph-name rows to remap.
            if self.quads + self.graph_names > 0 { return Err(bad("deduplicated build with named graphs")) }
            remap_dedup(&mut input, &shards, &mut triples, self.triples)?;
        } else {
        for row in 0..rows {
            let arity = if row < self.triples { 3 } else if row < self.triples + self.quads { 4 } else { 1 };
            let mut current_id = None;
            let mut ids = [[0u8; 8]; 4];
            for pos in 0..arity as u8 {
                let mut r = [0u8; 17];
                if !read_fixed(&mut input, &mut r)? { return Err(bad("missing term assignment")) }
                if r[8] != pos { return Err(bad("invalid term assignment")); }
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
        // Only row-less part records (RDF 1.2 triple-term parts, `push_parts`) may follow the rows; they sort last.
        let mut extra = [0u8; 17];
        while read_fixed(&mut input, &mut extra)? {
            let free = cfg!(feature = "rdf-12") && extra[..8] == FREE_ROW.to_be_bytes() && extra[8] == FREE_POS;
            if !free { return Err(bad("extra term assignment")) }
        }
        }
        triples.flush()?;
        drop(triples);
        drop(input);
        fs::remove_file(&assignments)?;
        checkpoint(2)?;
        let dictionary_identity = super::lazy_dictionary::convert_unified(dir)?;
        checkpoint(3)?;
        let mut identities = vec![dictionary_identity];
        let orders = self.orders;
        let kept = |&(slot, _): &(usize, _)| orders & (1 << slot) != 0;
        let jobs: Vec<_> = FILES[1..].iter().copied().zip(Perm::ALL).enumerate().filter(kept).map(|(_, j)| j).collect();
        let workers = self.index_workers;
        let run_budget = (budget_mb().saturating_mul(1_048_576) / workers).min(MEMORY);
        for batch in jobs.chunks(workers) {
            let results = std::thread::scope(|scope| {
                let handles: Vec<_> = batch.iter().map(|&(name, perm)| {
                    let raw = &raw; let triples = self.triples;
                    let format = super::disk_index::index_format_of_this_thread();
                    scope.spawn(move || { super::disk_index::set_index_format_of_this_thread(format); build_permutation(dir, raw, triples, name, perm, run_budget) })
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
            let quad_jobs: Vec<_> = QUAD_FILES.iter().copied().zip(Perm::ALL).enumerate().filter(kept).map(|(_, j)| j).collect();
            for batch in quad_jobs.chunks(workers) {
                let results = std::thread::scope(|scope| {
                    let handles: Vec<_> = batch.iter().map(|&(name, perm)| {
                        let raw = &quad_raw; let quads = self.quads;
                        let format = super::disk_index::index_format_of_this_thread();
                        scope.spawn(move || { super::disk_index::set_index_format_of_this_thread(format); build_quad_permutation(dir, raw, quads, name, perm, run_budget) })
                    }).collect();
                    handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(io::Error::other("permutation build worker panicked")))).collect::<Vec<_>>()
                });
                for result in results { identities.push(result?); }
            }
            fs::remove_file(&quad_raw)?;
            identities.push(write_catalog(dir, graph_ids)?);
        }
        // RDFSNAP7: the triple-term parts table is the last identity.
        #[cfg(feature = "rdf-12")]
        if rdf12 { identities.push(write_triple_terms(dir)?) }
        // Finish and remove all scratch before publishing the manifest.
        drop(mapping);
        drop(self.terms);
        let mut manifest = BufWriter::new(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(dir.join("MANIFEST"))?,
        );
        // RDFSNAP5 (task #36) = RDFSNAP4 plus six quad orders and the graph catalog. RDFSNAP6 (task #73) = either,
        // with only the kept orders, after the order set and the named-graph flag.
        // RDFSNAP7 (task #101): the RDFSNAP6 layout for any order set, for a base holding RDF 1.2 terms. A base without
        // one is written exactly as before.
        if rdf12 {
            manifest.write_all(b"RDFSNAP7")?;
            manifest.write_all(&[orders, u8::from(named)])?;
        } else if orders == super::base_format::ALL_ORDERS {
            manifest.write_all(if named { b"RDFSNAP5" } else { b"RDFSNAP4" })?;
        } else {
            manifest.write_all(b"RDFSNAP6")?;
            manifest.write_all(&[orders, u8::from(named)])?;
        }
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
/// The distinct rows of a sorted `FixedSort` output of `W` = `8 * N`-byte records, read without a per-row allocation.
struct UniqueRows<const N: usize, const W: usize> {
    input: BufReader<File>,
    previous: Option<[u8; W]>,
    done: bool,
}
impl<const N: usize, const W: usize> UniqueRows<N, W> {
    fn open(path: &Path) -> io::Result<Self> {
        Ok(Self {
            input: BufReader::with_capacity(64 * 1024, File::open(path)?),
            previous: None,
            done: false,
        })
    }
}
impl<const N: usize, const W: usize> Iterator for UniqueRows<N, W> {
    type Item = io::Result<[TermId; N]>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let mut bytes = [0u8; W];
        loop {
            match read_fixed(&mut self.input, &mut bytes) {
                Ok(true) => {
                    if self.previous == Some(bytes) {
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
                Ok(false) => {
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
pub(super) fn from_nt_parallel(dir: &Path, path: Option<&Path>, workers: usize) -> io::Result<()> {
    from_nt_parallel_with(dir, path, workers, dedup_setting())
}
fn dedup_setting() -> Dedup {
    if std::env::var("TRIPLELIGHT_DEDUP_TERMS").as_deref() == Ok("0") { return Dedup::Off }
    match std::env::var("TRIPLELIGHT_DEDUP_MAP_MB").ok().and_then(|v| v.parse::<usize>().ok()) {
        Some(mb) => Dedup::Map(mb.max(1).saturating_mul(1_048_576)),
        None => Dedup::Auto,
    }
}
/// The deduplicating parse (task #91, `docs/dedup-build-v1`): each shard replaces repeated term texts by shard-local
/// IDs before the term sort. On by default; `TRIPLELIGHT_DEDUP_TERMS=0` turns it off (an escape hatch kept for one
/// release), and `TRIPLELIGHT_DEDUP_MAP_MB=n` fixes the map budget per shard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Dedup {
    /// The plain parse: one sort record per term occurrence.
    Off,
    /// Each shard's term map gets what its share of the build budget leaves after its term sorter, capped at
    /// `DEDUP_MAP_MAX`, so map and sort buffers together stay within `--build-memory-mb` (cgroup-aware since #148). A
    /// smaller map only flushes more often. Below `DEDUP_MAP_MIN` the shard parses plainly.
    Auto,
    /// A fixed map budget per shard, in bytes.
    Map(usize),
}
const DEDUP_MAP_MAX: usize = 256 * 1024 * 1024;
const DEDUP_MAP_MIN: usize = 1024 * 1024;
pub(super) fn from_nt_parallel_with(dir: &Path, path: Option<&Path>, workers: usize, dedup: Dedup) -> io::Result<()> {
    let Some(path) = path else { return from_nt(dir, None); };
    // A worker's merge must hold at least two maximum-length records and file buffers.
    // The global limit is an advisory allocation target, not an RSS guarantee.
    let budget = budget_mb().saturating_mul(1_048_576);
    let minimum = 2 * (TERM_SORT_RECORD_MAX + 8192 + 128);
    let shards = workers.clamp(1, 8).min((budget / minimum).max(1));
    if shards == 1 { return from_nt(dir, Some(path)); }
    // One shard's share of the budget: its term sorter first (as before), then its term map.
    let share = budget / shards;
    let worker_budget = share.min(MEMORY);
    let dedup = match dedup {
        Dedup::Off => None,
        Dedup::Map(bytes) => Some(bytes),
        Dedup::Auto => Some(share.saturating_sub(worker_budget).min(DEDUP_MAP_MAX)).filter(|&m| m >= DEDUP_MAP_MIN),
    };
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
            handles.push(scope.spawn(move || -> io::Result<(Builder, std::path::PathBuf, Option<DedupShard>)> {
                let mut b = Builder::with_budget(dir, worker_budget)?;
                let mut file = File::open(path)?; file.seek(SeekFrom::Start(start))?;
                let input = BufReader::new(file.take(end - start));
                if let Some(map_bytes) = dedup {
                    let shard_rows = dedup_shard(dir, shard, &mut b, input, map_bytes)?;
                    let sorted = b.terms.finish()?;
                    return Ok((b, sorted, Some(shard_rows)));
                }
                for t in triplelight_core::ntriples::read_ntriples(input) {
                    let t: Triple = t?;
                    if b.triples >= (1u64 << 56) { return Err(bad("too many triples in shard")); }
                    let id = ((shard as u64) << 56) | b.triples;
                    b.add_with_id([t.subject.to_string(), t.predicate.to_string(), t.object.to_string()], id)?;
                }
                let sorted = b.terms.finish()?;
                Ok((b, sorted, None))
            }));
        }
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(io::Error::other("N-Triples worker panicked")))).collect::<Vec<_>>()
    });
    let mut builders = Vec::new();
    let mut dedup_rows = Vec::new();
    for r in results {
        let (b, path, rows) = r?;
        builders.push((b, path));
        dedup_rows.extend(rows);
    }
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
    if dedup.is_some() { combined.dedup_rows = Some(dedup_rows); }
    combined.index_workers = shards.min(3);
    drop(builders);
    combined.finish(dir)
}

/// RDF 1.2 (task #101): the `triple_terms` table of a just-written dictionary, one row `(term, s, p, o)` per triple
/// term in ID order, so `Dictionary::triple_term_parts` is one block read. Every part is a dictionary term
/// (`Builder::push_parts`). Kind-4 IDs ascend with the dictionary's ID order, so rows come out sorted.
#[cfg(feature = "rdf-12")]
fn write_triple_terms(dir: &Path) -> io::Result<[u8; 32]> {
    use super::lazy_dictionary::{LazyIndex, Order, UNIFIED_FILE};
    let dictionary = LazyIndex::open(&dir.join(UNIFIED_FILE), Order::Unified)?;
    let id_of = |term: Term| -> io::Result<TermId> {
        dictionary.find_term(term.to_string().as_bytes())?.map(|v| v.0).ok_or_else(|| bad("triple-term part missing from the dictionary"))
    };
    let mut rows: Vec<[TermId; 4]> = Vec::new();
    dictionary.for_each_of_kind(crate::model::id_tag::TRIPLE_TERM, &mut |id, term| {
        let Term::Triple(t) = term else { return Err(bad("kind 4 entry is not a triple term")) };
        let t = (**t).clone();
        rows.push([id, id_of(t.subject.into())?, id_of(t.predicate.into())?, id_of(t.object)?]);
        Ok(())
    })?;
    let path = dir.join(super::base_format::TRIPLE_TERMS_FILE);
    let mut out = BufWriter::new(OpenOptions::new().write(true).create_new(true).open(&path)?);
    super::disk_index::write_index::<4>(&mut out, rows.into_iter())?;
    out.flush()?;
    out.get_ref().sync_all()?;
    drop(out);
    let index = super::disk_index::DiskIndex::<4>::open(&path)?;
    index.validate()?;
    Ok(index.identity())
}
/// One shard of a deduplicating parse (task #91). Each distinct term gets a local ID, dense from 0 in order of first
/// appearance, and each triple is written as three big-endian local IDs. When the term map outgrows `map_bytes` it is
/// pushed into the shard's sorter as records `(text, 0, shard << 56 | local ID, 0)` and cleared, so a term seen again
/// gets a new local ID and the global dictionary pass still sees each text with every ID it was given.
fn dedup_shard(dir: &Path, shard: usize, b: &mut Builder, input: impl BufRead, map_bytes: usize) -> io::Result<DedupShard> {
    use std::collections::HashMap;
    let rows_path = dir.join(format!(".rows-{shard}"));
    let mut rows = BufWriter::with_capacity(1 << 20, OpenOptions::new().write(true).create_new(true).open(&rows_path)?);
    let mut map: HashMap<String, u64> = HashMap::new();
    let (mut used, mut next, mut flushes, mut sorted_bytes) = (0usize, 0u64, 0u64, 0u64);
    let flush = |map: &mut HashMap<String, u64>, b: &mut Builder, sorted_bytes: &mut u64| -> io::Result<()> {
        for (text, local) in map.drain() {
            let mut r = text.into_bytes();
            r.push(0);
            r.extend_from_slice(&(((shard as u64) << 56) | local).to_be_bytes());
            r.push(0);
            *sorted_bytes += r.len() as u64;
            b.terms.push(r)?;
        }
        Ok(())
    };
    for t in triplelight_core::ntriples::read_ntriples(input) {
        let t: Triple = t?;
        if b.triples >= (1u64 << 56) { return Err(bad("too many triples in shard")); }
        for term in [t.subject.to_string(), t.predicate.to_string(), t.object.to_string()] {
            if term.len() > MAX_TERM || term.as_bytes().contains(&0) {
                return Err(bad("invalid or oversized encoded RDF term"));
            }
            // RDF 1.2: a triple term's parts become dictionary terms too, as row-less records (`push_parts`).
            #[cfg(feature = "rdf-12")]
            if term.starts_with("<<(") && !map.contains_key(term.as_str()) { b.push_parts(&term)? }
            let local = match map.get(term.as_str()) {
                Some(&local) => local,
                None => {
                    let local = next;
                    next += 1;
                    used += term.len();
                    map.insert(term, local);
                    local
                }
            };
            rows.write_all(&local.to_be_bytes())?;
        }
        b.triples += 1;
        // Text bytes plus the table itself: each bucket holds a `String` header (24 bytes) and a `u64`, plus one control
        // byte, and the bucket array grows by powers of two, so charge its capacity rather than its length.
        if used + map.capacity() * 33 > map_bytes {
            flush(&mut map, b, &mut sorted_bytes)?;
            flushes += 1;
            used = 0;
        }
    }
    flush(&mut map, b, &mut sorted_bytes)?;
    rows.flush()?;
    // `flushes` counts the map evictions during the parse (the final drain is not one).
    if std::env::var_os("TRIPLELIGHT_DEDUP_TRACE").is_some() {
        eprintln!("[dedup shard {shard}] triples={} local_ids={next} flushes={flushes} record_bytes_to_sort={sorted_bytes}", b.triples);
    }
    Ok(DedupShard { rows: rows_path, locals: next, triples: b.triples })
}
/// Writes `.triples.raw` rows from the dedup rows files: `assignments` holds one 17-byte record per local ID
/// `(shard << 56 | local, 0, global ID)`, sorted by local ID, so each shard's IDs arrive dense and in order and its
/// local-to-global table is read into memory.
fn remap_dedup(assignments: &mut impl Read, shards: &[DedupShard], out: &mut impl Write, triples: u64) -> io::Result<()> {
    const LOCAL: u64 = (1 << 56) - 1;
    let mut table: Vec<u64> = Vec::new();
    let mut done = 0usize;
    let mut total = 0u64;
    let finish_shard = |s: usize, table: &Vec<u64>, out: &mut dyn Write| -> io::Result<u64> {
        let sh = &shards[s];
        if table.len() as u64 != sh.locals { return Err(bad("dedup assignments do not cover every local ID")); }
        let mut input = BufReader::with_capacity(1 << 20, File::open(&sh.rows)?);
        let mut row = [0u8; 24];
        for _ in 0..sh.triples {
            input.read_exact(&mut row)?;
            for k in 0..3 {
                let local = u64::from_be_bytes(row[k * 8..k * 8 + 8].try_into().unwrap()) as usize;
                let global = *table.get(local).ok_or_else(|| bad("dedup row references an unknown local ID"))?;
                out.write_all(&global.to_be_bytes())?;
            }
        }
        fs::remove_file(&sh.rows)?;
        Ok(sh.triples)
    };
    // The mapping is a `FixedSort<17>` output since #90: raw 17-byte records, no length prefix.
    let mut r = [0u8; 17];
    while read_fixed(assignments, &mut r)? {
        let id = u64::from_be_bytes(r[..8].try_into().unwrap());
        // Row-less RDF 1.2 part records (`push_parts`) sort after every row; they map to no row.
        if cfg!(feature = "rdf-12") && id == FREE_ROW && r[8] == FREE_POS { continue }
        if r[8] != 0 { return Err(bad("invalid term assignment")); }
        let (shard, local) = ((id >> 56) as usize, id & LOCAL);
        if shard >= shards.len() || shard < done { return Err(bad("unordered term assignment")); }
        while done < shard { total += finish_shard(done, &table, out)?; table.clear(); done += 1; }
        if local != table.len() as u64 { return Err(bad("unordered term assignment")); }
        table.push(u64::from_be_bytes(r[9..17].try_into().unwrap()));
    }
    while done < shards.len() { total += finish_shard(done, &table, out)?; table.clear(); done += 1; }
    if total != triples { return Err(bad("dedup triple count mismatch")); }
    Ok(())
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
    from_dataset_keeping(dir, source, kept_orders_setting())
}
/// `from_dataset` writing the orders `orders` (bits of `Perm::ALL` positions) instead of the process setting.
pub(super) fn from_dataset_keeping(dir: &Path, source: &dyn triplelight_core::storage::DatasetView, orders: u8) -> io::Result<()> {
    let names = source.graph_names()?;
    if names.is_empty() { return from_snapshot_keeping(dir, source.default_graph(), orders) }
    fs::create_dir(dir)?;
    let mut builder = Builder::new(dir)?;
    builder.orders = orders;
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
    from_snapshot_keeping(dir, source, kept_orders_setting())
}
pub(super) fn from_snapshot_keeping(dir: &Path, source: &dyn Snapshot, orders: u8) -> io::Result<()> {
    fs::create_dir(dir)?;
    let mut builder = Builder::new(dir)?;
    builder.orders = orders;
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


/// Unit tests of the builder's internals; tests that read a built base back are in `external_build_tests`.
#[cfg(test)]
mod tests {
    use super::*;
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
}
