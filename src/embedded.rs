//! In-process persistent RDF database, without an HTTP server.
//!
//! One process may open a database directory at a time. Clone the `Arc<Database>`
//! held by the application to share it among threads. The directory lock is
//! advisory: older programs that do not honor it must not open the same store.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use crate::opt::stats::Stats;
use crate::exec::{self, QueryError, QueryResult};
use crate::store::{Change, Overlay, Store, StoreUpdate};
use crate::storage::{Dictionary, DiskBase, MemorySnapshot, Snapshot};
use crate::model::Perm;

/// Errors returned by embedded operations.
#[derive(Debug)]
pub enum DatabaseError { Syntax(String), Unsupported(String), Storage(std::io::Error), Locked }
impl std::fmt::Display for DatabaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { Self::Syntax(s) | Self::Unsupported(s) => f.write_str(s), Self::Storage(e) => e.fmt(f), Self::Locked => f.write_str("database directory is already open") }
    }
}
impl std::error::Error for DatabaseError {}
impl From<std::io::Error> for DatabaseError {
    fn from(e: std::io::Error) -> Self { if e.kind() == std::io::ErrorKind::WouldBlock { Self::Locked } else { Self::Storage(e) } }
}
impl From<QueryError> for DatabaseError {
    fn from(e: QueryError) -> Self {
        match e.kind {
            exec::QueryErrorKind::Syntax => Self::Syntax(e.message),
            exec::QueryErrorKind::Unsupported => Self::Unsupported(e.message),
            exec::QueryErrorKind::Storage => Self::Storage(std::io::Error::other(e.message)),
        }
    }
}

/// Options for opening a persistent on-disk database.
#[derive(Debug, Clone)]
pub struct DatabaseOptions {
    /// N-Triples input used only when creating a new database.
    pub initial: Option<PathBuf>,
    /// Number of workers used when initially importing a database.
    pub import_workers: usize,
}
impl Default for DatabaseOptions {
    fn default() -> Self {
        Self { initial: None, import_workers: std::thread::available_parallelism().map_or(1, |n| n.get()) }
    }
}

/// An embedded RDF database. Each operation observes a consistent snapshot.
/// Persistent updates are acknowledged after the write-ahead log is synchronized; in-memory updates are not durable.
#[derive(Clone)]
pub struct Database {
    // Declared first so it drops first: closing waits for an in-flight statistics write, then the Store (and its
    // directory lock) is released, so the directory can be reopened as soon as the last handle is dropped.
    closer: Arc<CloseGate>,
    store: Backend, stats: Arc<RwLock<Option<(Arc<Overlay>, Arc<Stats>)>>>, origin: Arc<Mutex<Option<StatsOrigin>>> }

/// Set when the last `Database` handle is dropped; a background statistics writer checks it (under the same lock)
/// before touching the directory, so nothing is written after close and close waits for a write in progress.
struct CloseGate(Arc<Mutex<bool>>);
impl Drop for CloseGate { fn drop(&mut self) { *self.0.lock().unwrap() = true } }

/// Where the planner statistics for the current base came from (diagnostics and measurement).
#[derive(Debug, Clone, PartialEq)]
pub enum StatsOrigin {
    /// Loaded at open from the base's persisted statistics file: its size and the read + verify + decode time.
    Persisted { bytes: u64, load_ms: f64 },
    /// Built by a background scan of the base; `persisted` tells whether a statistics file was then written.
    Scanned { scan_ms: f64, persisted: bool },
}

#[derive(Clone)]
enum Backend { Disk(Arc<Store>), Memory(Arc<MemoryStore>) }
struct MemoryStore { current: RwLock<Arc<Overlay>>, writer: Mutex<()> }

impl Database {
    /// Opens an existing database, or creates an empty database at `dir`.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        Self::open_with(dir, DatabaseOptions::default())
    }

    /// Opens a database, optionally initializing a new one from an N-Triples file.
    /// The initial file is ignored when the directory already holds a database.
    /// Opening an already open directory fails instead of allowing two writers.
    pub fn open_with(dir: impl AsRef<Path>, options: DatabaseOptions) -> Result<Self, DatabaseError> {
        if options.import_workers == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "import_workers must be positive").into());
        }
        let format = DiskBase { workers: options.import_workers };
        let store = Arc::new(Store::open_with(dir, options.initial.as_deref(), Box::new(format))?);
        let db = Self { closer: Arc::new(CloseGate(Arc::new(Mutex::new(false)))), store: Backend::Disk(store), stats: Arc::new(RwLock::new(None)), origin: Arc::new(Mutex::new(None)) };
        if !db.load_persisted_stats()? { db.refresh_stats(); }
        Ok(db)
    }

    /// Creates an isolated, non-durable in-memory database. Clones share the same state.
    pub fn in_memory() -> Self {
        let base = MemorySnapshot::from_triples(Vec::new());
        let current = Arc::new(Overlay::from_base(Arc::new(base)));
        let db = Self { closer: Arc::new(CloseGate(Arc::new(Mutex::new(false)))), store: Backend::Memory(Arc::new(MemoryStore { current: RwLock::new(current), writer: Mutex::new(()) })), stats: Arc::new(RwLock::new(None)), origin: Arc::new(Mutex::new(None)) };
        db.refresh_stats();
        db
    }

    fn snapshot(&self) -> Arc<Overlay> {
        match &self.store { Backend::Disk(s) => s.snapshot(), Backend::Memory(s) => s.current.read().unwrap().clone() }
    }

    /// Statistics for the current base from its persisted file, if one exists and verifies. `Ok(false)` means
    /// the file is missing or unusable (the caller rescans); errors reading the base itself are returned.
    fn load_persisted_stats(&self) -> Result<bool, DatabaseError> {
        let Backend::Disk(store) = &self.store else { return Ok(false) };
        let start = std::time::Instant::now();
        let (generation, snap) = store.generation_snapshot();
        let base_triples = snap.base().count(Perm::Spo, &[])?;
        let Some(loaded) = store.with_generation(generation, |dir| stats_file::load(dir, generation, base_triples)).flatten() else { return Ok(false) };
        let (stats, bytes) = loaded;
        *self.stats.write().unwrap() = Some((snap, Arc::new(stats)));
        *self.origin.lock().unwrap() = Some(StatsOrigin::Persisted { bytes, load_ms: start.elapsed().as_secs_f64() * 1e3 });
        Ok(true)
    }

    fn refresh_stats(&self) {
        let target = Arc::clone(&self.stats);
        let origin = Arc::clone(&self.origin);
        match &self.store {
            Backend::Memory(store) => {
                let snap = self.snapshot();
                let store = Arc::clone(store);
                std::thread::spawn(move || {
                    let start = std::time::Instant::now();
                    if let Ok(st) = Stats::from_snapshot(snap.base()) {
                        let current = store.current.read().unwrap();
                        if current.base_id() == snap.base_id() {
                            *target.write().unwrap() = Some((snap, Arc::new(st)));
                            *origin.lock().unwrap() = Some(StatsOrigin::Scanned { scan_ms: start.elapsed().as_secs_f64() * 1e3, persisted: false });
                        }
                    }
                });
            }
            Backend::Disk(store) => {
                // The scan keeps its snapshot (the base files) but not the Store: holding only a Weak, it cannot keep
                // the directory lock after the database is closed, and it writes nothing once it has been.
                let (generation, snap) = store.generation_snapshot();
                let weak = Arc::downgrade(store);
                let gate = Arc::clone(&self.closer.0);
                std::thread::spawn(move || {
                    let start = std::time::Instant::now();
                    let Ok(st) = Stats::from_snapshot(snap.base()) else { return };
                    let scan_ms = start.elapsed().as_secs_f64() * 1e3;
                    let closed = gate.lock().unwrap();
                    if *closed { return }
                    let Some(store) = weak.upgrade() else { return };
                    // A scan of an older base (compaction happened meanwhile) is neither installed nor persisted.
                    if store.snapshot().base_id() != snap.base_id() { return }
                    let st = Arc::new(st);
                    let persisted = match snap.base().count(Perm::Spo, &[]) {
                        Ok(n) => store.with_generation(generation, |dir| stats_file::store(dir, generation, &st, n).is_ok()).unwrap_or(false),
                        Err(_) => false,
                    };
                    *target.write().unwrap() = Some((snap, st));
                    *origin.lock().unwrap() = Some(StatsOrigin::Scanned { scan_ms, persisted });
                    drop(store); // before `closed`: a waiting close then frees the Store and its directory lock
                    drop(closed);
                });
            }
        }
    }

    /// Where the current statistics came from, once they are available (see `stats_ready`).
    pub fn stats_origin(&self) -> Option<StatsOrigin> { self.origin.lock().unwrap().clone() }
    /// Whether statistics for the currently visible unmodified base are ready.
    pub fn stats_ready(&self) -> bool {
        let snap = self.snapshot();
        self.stats.read().unwrap().as_ref().is_some_and(|(base, _)| snap.delta_len() == 0 && base.base_id() == snap.base_id())
    }

    /// Executes a supported SPARQL query, returning owned RDF terms.
    pub fn query(&self, sparql: &str) -> Result<QueryResult, DatabaseError> {
        self.read().query(sparql)
    }

    /// Returns a stable read view that is unaffected by subsequent updates.
    pub fn read(&self) -> ReadSnapshot { ReadSnapshot { snapshot: self.snapshot(), stats: Arc::clone(&self.stats) } }

    /// Applies a supported SPARQL Update atomically. Persistent updates are durable; in-memory updates are not.
    pub fn update(&self, sparql: &str) -> Result<usize, DatabaseError> {
        match &self.store {
            Backend::Disk(store) => Ok(store.update(sparql)?),
            Backend::Memory(store) => {
                let _writer = store.writer.lock().unwrap();
                let current = store.current.read().unwrap().clone();
                let changes = exec::update_changes(&*current, sparql)?;
                let (next, count) = current.with_changes(&changes)?;
                *store.current.write().unwrap() = Arc::new(next);
                Ok(count)
            }
        }
    }

    /// Serializes SELECT or ASK as SPARQL Results JSON with batched term decoding.
    pub fn query_json(&self, sparql: &str) -> Result<String, DatabaseError> { self.read().query_json(sparql) }

    /// Atomically imports an N-Triples (.nt) or Turtle (.ttl) file into this database.
    /// Parsing and indexing complete before committing; failures leave the graph unchanged.
    pub fn load(&self, path: impl AsRef<Path>) -> Result<usize, DatabaseError> {
        let path = path.as_ref();
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if let Backend::Memory(store) = &self.store {
            let writer = store.writer.lock().unwrap();
            let current = store.current.read().unwrap().clone();
            if current.count(Perm::Spo, &[])? == 0 {
                let reader = std::io::BufReader::new(std::fs::File::open(path)?);
                let mut scope = BlankScope::default();
                let built = match ext {
                    "nt" => MemorySnapshot::try_from_triples(oxttl::NTriplesParser::new().for_reader(reader).map(|t| t.map(|t| scope.rewrite(t)))),
                    "ttl" => MemorySnapshot::try_from_triples(oxttl::TurtleParser::new().for_reader(reader).map(|t| t.map(|t| scope.rewrite(t)))),
                    _ => return Err(DatabaseError::Unsupported("load requires an .nt or .ttl file".into())),
                }.map_err(|e| match e {
                    oxttl::TurtleParseError::Io(e) => DatabaseError::Storage(e),
                    oxttl::TurtleParseError::Syntax(e) => DatabaseError::Syntax(e.to_string()),
                })?;
                let count = built.count(Perm::Spo, &[])?;
                *store.current.write().unwrap() = Arc::new(Overlay::from_base(Arc::new(built)));
                drop(writer);
                self.refresh_stats();
                return Ok(count as usize);
            }
        }
        let reader = std::io::BufReader::new(std::fs::File::open(path)?);
        let triples = match ext {
            "nt" => oxttl::NTriplesParser::new().for_reader(reader).collect::<Result<Vec<_>, _>>(),
            "ttl" => oxttl::TurtleParser::new().for_reader(reader).collect::<Result<Vec<_>, _>>(),
            _ => return Err(DatabaseError::Unsupported("load requires an .nt or .ttl file".into())),
        }.map_err(|e| match e {
            oxttl::TurtleParseError::Io(e) => DatabaseError::Storage(e),
            oxttl::TurtleParseError::Syntax(e) => DatabaseError::Syntax(e.to_string()),
        })?;
        let triples = scope_blank_nodes(triples);
        match &self.store {
            Backend::Disk(store) => {
                let changes: Vec<_> = triples.into_iter().map(|t| (Change::Insert, t)).collect();
                Ok(store.commit(&changes)?)
            }
            Backend::Memory(store) => {
                let _writer = store.writer.lock().unwrap();
                let current = store.current.read().unwrap().clone();
                if current.count(Perm::Spo, &[])? == 0 {
                    let base = MemorySnapshot::from_triples(triples);
                    let count = base.count(Perm::Spo, &[])?;
                    *store.current.write().unwrap() = Arc::new(Overlay::from_base(Arc::new(base)));
                    drop(_writer);
                    self.refresh_stats();
                    Ok(count as usize)
                } else {
                    let changes: Vec<_> = triples.into_iter().map(|t| (Change::Insert, t)).collect();
                    let (next, count) = current.with_changes(&changes)?;
                    *store.current.write().unwrap() = Arc::new(next);
                    Ok(count)
                }
            }
        }
    }

    /// Writes the current state into a new immutable base; existing readers retain their snapshot.
    pub fn compact(&self) -> Result<(), DatabaseError> {
        match &self.store {
            Backend::Disk(store) => store.compact()?,
            Backend::Memory(store) => {
                let _writer = store.writer.lock().unwrap();
                let current = store.current.read().unwrap().clone();
                let mut triples = Vec::new();
                for batch in current.scan(Perm::Spo, &[]) {
                    let batch = batch?;
                    for i in 0..batch.len() {
                        let subject: crate::rdf::NamedOrBlankNode = current.resolve(batch.cols[0][i])?.try_into()
                            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid RDF subject"))?;
                        let predicate: crate::rdf::NamedNode = current.resolve(batch.cols[1][i])?.try_into()
                            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid RDF predicate"))?;
                        let object = current.resolve(batch.cols[2][i])?;
                        triples.push(crate::rdf::Triple::new(subject, predicate, object));
                    }
                }
                let base = MemorySnapshot::from_triples(triples);
                *store.current.write().unwrap() = Arc::new(Overlay::from_base(Arc::new(base)));
            }
        }
        self.refresh_stats(); Ok(())
    }
}

// Blank node labels are local to each imported document.
#[derive(Default)]
struct BlankScope(std::collections::HashMap<crate::rdf::BlankNode, crate::rdf::BlankNode>);
impl BlankScope {
    fn rewrite(&mut self, t: crate::rdf::Triple) -> crate::rdf::Triple {
        use crate::rdf::{NamedOrBlankNode, Term, Triple};
        let mut fresh = |b: &crate::rdf::BlankNode| self.0.entry(b.clone()).or_default().clone();
        let subject = match t.subject { NamedOrBlankNode::BlankNode(b) => NamedOrBlankNode::BlankNode(fresh(&b)), s => s };
        let object = match t.object { Term::BlankNode(b) => Term::BlankNode(fresh(&b)), o => o };
        Triple::new(subject, t.predicate, object)
    }
}
fn scope_blank_nodes(triples: Vec<crate::rdf::Triple>) -> Vec<crate::rdf::Triple> {
    let mut scope = BlankScope::default();
    triples.into_iter().map(|t| scope.rewrite(t)).collect()
}

/// An immutable view of the database at the instant `Database::read` was called.
pub struct ReadSnapshot { snapshot: Arc<Overlay>, stats: Arc<RwLock<Option<(Arc<Overlay>, Arc<Stats>)>>> }
impl ReadSnapshot {
    pub fn query(&self, sparql: &str) -> Result<QueryResult, DatabaseError> {
        let stats = self.applicable_stats();
        let opts = exec::Options { stats: stats.as_deref(), ..Default::default() };
        Ok(exec::query_with(&*self.snapshot, sparql, &opts)?)
    }
    fn applicable_stats(&self) -> Option<Arc<Stats>> {
        if self.snapshot.delta_len() != 0 { return None; }
        self.stats.read().unwrap().as_ref().filter(|(base, _)| base.base_id() == self.snapshot.base_id()).map(|(_, st)| Arc::clone(st))
    }
    /// Executes a query with undecoded SELECT IDs, valid only against this snapshot.
    pub fn query_ids(&self, sparql: &str) -> Result<QueryResult, DatabaseError> {
        let stats = self.applicable_stats();
        let opts = exec::Options { stats: stats.as_deref(), raw_ids: true, ..Default::default() };
        Ok(exec::query_with(&*self.snapshot, sparql, &opts)?)
    }
    /// Decodes stored term IDs returned by `query_ids` on this snapshot in one batched dictionary pass, calling
    /// `f(i, term)` for `ids[i]` (in any order). Computed terms are not stored: look them up in
    /// `IdSolutions::scratch` instead.
    pub fn with_terms(&self, ids: &[crate::model::TermId], f: &mut dyn FnMut(usize, crate::rdf::TermRef<'_>)) -> Result<(), DatabaseError> {
        Ok(self.snapshot.with_terms(ids, f)?)
    }
    /// Serializes SELECT IDs returned by this snapshot with batched dictionary access.
    pub fn serialize_ids(&self, ids: &exec::IdSolutions) -> Result<String, DatabaseError> {
        Ok(exec::results::serialize_ids(ids, &*self.snapshot)?)
    }
    /// Serializes SELECT or ASK as SPARQL Results JSON; graph results are not JSON results.
    pub fn query_json(&self, sparql: &str) -> Result<String, DatabaseError> {
        let result = self.query_ids(sparql)?;
        match &result {
            QueryResult::Ids(ids) => self.serialize_ids(ids),
            QueryResult::Boolean(_) | QueryResult::Solutions { .. } => Ok(exec::results::serialize(&result).1),
            QueryResult::Graph(_) => Err(DatabaseError::Unsupported("query_json supports SELECT and ASK only".into())),
        }
    }
}

#[cfg(test)]
mod memory_tests {
    use super::*;
    #[test]
    fn reader_does_not_wait_for_writer() {
        let db = Database::in_memory();
        let Backend::Memory(store) = &db.store else { unreachable!() };
        let _writer = store.writer.lock().unwrap();
        let copy = db.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || { tx.send(copy.read()).unwrap(); });
        let result = rx.recv_timeout(std::time::Duration::from_secs(2));
        drop(_writer);
        thread.join().unwrap();
        assert!(result.is_ok(), "reader blocked behind writer mutex");
    }
}

/// Persisted planner statistics: `<db>/stats-<generation>.v1` beside the immutable `base-<generation>/` it describes.
///
/// Layout: `TLSIDE01`, generation (u64 LE), sha256 of the exact `base-<generation>/MANIFEST` bytes, payload length
/// (u64 LE), sha256 of the payload, payload (`Stats::encode`). The file is disposable: anything that does not verify
/// is ignored (the caller rescans) and replaced by the next scan.
mod stats_file {
    use super::Stats;
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use std::path::{Path, PathBuf};

    const MAGIC: &[u8; 8] = b"TLSIDE01";
    const HEADER: usize = 8 + 8 + 32 + 8 + 32;
    /// Refuse absurd files before reading them into memory.
    const MAX_FILE: u64 = 1 << 30;

    fn path(dir: &Path, generation: u64) -> PathBuf { dir.join(format!("stats-{generation}.v1")) }
    fn manifest_sha(dir: &Path, generation: u64) -> std::io::Result<[u8; 32]> {
        Ok(Sha256::digest(std::fs::read(dir.join(format!("base-{generation}")).join("MANIFEST"))?).into())
    }

    /// The statistics and the file size, or `None` if the file is missing, foreign, stale or malformed.
    pub(super) fn load(dir: &Path, generation: u64, base_triples: u64) -> Option<(Stats, u64)> {
        let p = path(dir, generation);
        if std::fs::metadata(&p).ok()?.len() > MAX_FILE { return None }
        let buf = std::fs::read(&p).ok()?;
        if buf.len() < HEADER || &buf[..8] != MAGIC { return None }
        let field = |a: usize| u64::from_le_bytes(buf[a..a + 8].try_into().unwrap());
        if field(8) != generation || buf[16..48] != manifest_sha(dir, generation).ok()? { return None }
        let len = usize::try_from(field(48)).ok()?;
        let payload = buf.get(HEADER..)?;
        if payload.len() != len || buf[56..88] != Sha256::digest(payload)[..] { return None }
        Some((Stats::decode(payload, base_triples).ok()?, buf.len() as u64))
    }

    /// Writes the file for `generation` (unique temp name, fsync, rename, fsync of the directory) and removes
    /// statistics files and temp files of other generations. The caller holds the store and its generation lock.
    pub(super) fn store(dir: &Path, generation: u64, stats: &Stats, base_triples: u64) -> std::io::Result<()> {
        if stats.triple_count != base_triples { return Err(std::io::Error::other("statistics do not describe this base")) }
        let payload = stats.encode();
        let mut out = Vec::with_capacity(HEADER + payload.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&generation.to_le_bytes());
        out.extend_from_slice(&manifest_sha(dir, generation)?);
        out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        out.extend_from_slice(&Sha256::digest(&payload));
        out.extend_from_slice(&payload);
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let tmp = dir.join(format!("stats-{generation}.v1.tmp-{}-{nonce}", std::process::id()));
        let result = (|| {
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            f.write_all(&out)?;
            f.sync_all()?;
            std::fs::rename(&tmp, path(dir, generation))?;
            std::fs::File::open(dir)?.sync_all()
        })();
        if result.is_err() { let _ = std::fs::remove_file(&tmp); }
        result?;
        let keep = format!("stats-{generation}.v1");
        for entry in std::fs::read_dir(dir)? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if name.starts_with("stats-") && name != keep && (name.ends_with(".v1") || name.contains(".v1.tmp-")) {
                let _ = std::fs::remove_file(dir.join(&name));
            }
        }
        Ok(())
    }
}
