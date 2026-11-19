//! In-process persistent RDF database, without an HTTP server.
//!
//! One process may open a database directory at a time. Clone the `Arc<Database>`
//! held by the application to share it among threads. The directory lock is
//! advisory: older programs that do not honor it must not open the same store.
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use crate::opt::stats::Stats;
use crate::exec::{self, QueryError, QueryResult};
use crate::store::{Change, Overlay, Store, StoreUpdate};
use crate::storage::{DiskBase, MemorySnapshot, Snapshot};
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
pub struct Database { store: Backend, stats: Arc<RwLock<Option<(Arc<Overlay>, Arc<Stats>)>>> }

#[derive(Clone)]
enum Backend { Disk(Arc<Store>), Memory(Arc<RwLock<Arc<Overlay>>>) }

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
        let db = Self { store: Backend::Disk(store), stats: Arc::new(RwLock::new(None)) };
        db.refresh_stats();
        Ok(db)
    }

    /// Creates an isolated, non-durable in-memory database. Clones share the same state.
    pub fn in_memory() -> Self {
        let base = MemorySnapshot::from_triples(Vec::new());
        let current = Arc::new(Overlay::from_base(Arc::new(base)));
        let db = Self { store: Backend::Memory(Arc::new(RwLock::new(current))), stats: Arc::new(RwLock::new(None)) };
        db.refresh_stats();
        db
    }

    fn snapshot(&self) -> Arc<Overlay> {
        match &self.store { Backend::Disk(s) => s.snapshot(), Backend::Memory(s) => s.read().unwrap().clone() }
    }

    fn refresh_stats(&self) {
        let snap = self.snapshot();
        let target = Arc::clone(&self.stats);
        std::thread::spawn(move || {
            if let Ok(st) = Stats::from_snapshot(snap.base()) {
                *target.write().unwrap() = Some((snap, Arc::new(st)));
            }
        });
    }
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
                let mut current = store.write().unwrap();
                let changes = exec::update_changes(&**current, sparql)?;
                let (next, count) = current.with_changes(&changes)?;
                *current = Arc::new(next);
                Ok(count)
            }
        }
    }

    /// Serializes SELECT or ASK as SPARQL Results JSON with batched term decoding.
    pub fn query_json(&self, sparql: &str) -> Result<String, DatabaseError> { self.read().query_json(sparql) }

    /// Atomically imports an N-Triples (.nt) or Turtle (.ttl) file into this database.
    /// The input is parsed in full before committing; memory grows with input size.
    pub fn load(&self, path: impl AsRef<Path>) -> Result<usize, DatabaseError> {
        let path = path.as_ref();
        let reader = std::io::BufReader::new(std::fs::File::open(path)?);
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
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
                let mut current = store.write().unwrap();
                if current.count(Perm::Spo, &[])? == 0 {
                    let base = MemorySnapshot::from_triples(triples);
                    let count = base.count(Perm::Spo, &[])?;
                    *current = Arc::new(Overlay::from_base(Arc::new(base)));
                    drop(current);
                    self.refresh_stats();
                    Ok(count as usize)
                } else {
                    let changes: Vec<_> = triples.into_iter().map(|t| (Change::Insert, t)).collect();
                    let (next, count) = current.with_changes(&changes)?;
                    *current = Arc::new(next);
                    Ok(count)
                }
            }
        }
    }

    /// Writes the current state into a new immutable base; existing readers retain their snapshot.
    pub fn compact(&self) -> Result<(), DatabaseError> {
        match &self.store {
            Backend::Disk(store) => store.compact()?,
            Backend::Memory(_) => return Err(DatabaseError::Unsupported("in-memory compaction is not supported".into())),
        }
        self.refresh_stats(); Ok(())
    }
}

// Blank node labels are local to each imported document.
fn scope_blank_nodes(triples: Vec<crate::rdf::Triple>) -> Vec<crate::rdf::Triple> {
    use crate::rdf::{BlankNode, NamedOrBlankNode, Term, Triple};
    use std::collections::HashMap;
    let mut nodes = HashMap::<BlankNode, BlankNode>::new();
    let mut fresh = |b: &BlankNode| nodes.entry(b.clone()).or_default().clone();
    triples.into_iter().map(|t| {
        let subject = match t.subject { NamedOrBlankNode::BlankNode(b) => NamedOrBlankNode::BlankNode(fresh(&b)), s => s };
        let object = match t.object { Term::BlankNode(b) => Term::BlankNode(fresh(&b)), o => o };
        Triple::new(subject, t.predicate, object)
    }).collect()
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
