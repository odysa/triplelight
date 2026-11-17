//! In-process persistent RDF database, without an HTTP server.
//!
//! One process may open a database directory at a time. Clone the `Arc<Database>`
//! held by the application to share it among threads. The directory lock is
//! advisory: older programs that do not honor it must not open the same store.
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use crate::opt::stats::Stats;
use crate::exec::{self, QueryError, QueryResult};
use crate::store::{Change, Overlay, Store};
use crate::storage::DiskBase;

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

/// A persistent local RDF database. Each operation observes a consistent snapshot.
/// Updates are acknowledged after the write-ahead log is synchronized.
#[derive(Clone)]
pub struct Database { store: Arc<Store>, stats: Arc<RwLock<Option<(Arc<Overlay>, Arc<Stats>)>>> }

impl Database {
    /// Opens an existing database, or creates an empty database at `dir`.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        Self::open_with(dir, DatabaseOptions::default())
    }

    /// Opens a database, optionally initializing a new one from an N-Triples file.
    /// Opening an already open directory fails instead of allowing two writers.
    pub fn open_with(dir: impl AsRef<Path>, options: DatabaseOptions) -> Result<Self, DatabaseError> {
        if options.import_workers == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "import_workers must be positive").into());
        }
        let format = DiskBase { workers: options.import_workers };
        let store = Arc::new(Store::open_with(dir, options.initial.as_deref(), Box::new(format))?);
        let db = Self { store, stats: Arc::new(RwLock::new(None)) };
        db.refresh_stats();
        Ok(db)
    }

    fn refresh_stats(&self) {
        let snap = self.store.snapshot();
        let target = Arc::clone(&self.stats);
        std::thread::spawn(move || {
            if let Ok(st) = Stats::from_snapshot(snap.base()) {
                *target.write().unwrap() = Some((snap, Arc::new(st)));
            }
        });
    }
    /// Whether statistics for the currently visible unmodified base are ready.
    pub fn stats_ready(&self) -> bool {
        let snap = self.store.snapshot();
        self.stats.read().unwrap().as_ref().is_some_and(|(base, _)| snap.delta_len() == 0 && base.base_id() == snap.base_id())
    }

    /// Executes a supported SPARQL query, returning owned RDF terms.
    pub fn query(&self, sparql: &str) -> Result<QueryResult, DatabaseError> {
        self.read().query(sparql)
    }

    /// Returns a stable read view that is unaffected by subsequent updates.
    pub fn read(&self) -> ReadSnapshot { ReadSnapshot { snapshot: self.store.snapshot(), stats: Arc::clone(&self.stats) } }

    /// Applies a supported SPARQL Update atomically and durably.
    pub fn update(&self, sparql: &str) -> Result<usize, DatabaseError> { Ok(self.store.update(sparql)?) }

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
        let changes: Vec<_> = triples.into_iter().map(|t| (Change::Insert, t)).collect();
        Ok(self.store.commit(&changes)?)
    }

    /// Writes the current state into a new immutable base; existing readers retain their snapshot.
    pub fn compact(&self) -> Result<(), DatabaseError> { self.store.compact()?; self.refresh_stats(); Ok(()) }
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
