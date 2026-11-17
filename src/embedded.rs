//! In-process persistent RDF database, without an HTTP server.
//!
//! One process may open a database directory at a time. Clone the `Arc<Database>`
//! held by the application to share it among threads. The directory lock is
//! advisory: older programs that do not honor it must not open the same store.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use crate::exec::{self, QueryError, QueryResult};
use crate::store::{Overlay, Store};
use crate::storage::DiskBase;

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
pub struct Database { store: Store }

impl Database {
    /// Opens an existing database, or creates an empty database at `dir`.
    pub fn open(dir: impl AsRef<Path>) -> std::io::Result<Self> {
        Self::open_with(dir, DatabaseOptions::default())
    }

    /// Opens a database, optionally initializing a new one from an N-Triples file.
    /// Opening an already open directory fails instead of allowing two writers.
    pub fn open_with(dir: impl AsRef<Path>, options: DatabaseOptions) -> std::io::Result<Self> {
        if options.import_workers == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "import_workers must be positive"));
        }
        let format = DiskBase { workers: options.import_workers };
        Ok(Self { store: Store::open_with(dir, options.initial.as_deref(), Box::new(format))? })
    }

    /// Executes a supported SPARQL query, returning owned RDF terms.
    pub fn query(&self, sparql: &str) -> Result<QueryResult, QueryError> {
        self.read().query(sparql)
    }

    /// Returns a stable read view that is unaffected by subsequent updates.
    pub fn read(&self) -> ReadSnapshot { ReadSnapshot { snapshot: self.store.snapshot() } }

    /// Applies a supported SPARQL Update atomically and durably.
    pub fn update(&self, sparql: &str) -> Result<usize, QueryError> { self.store.update(sparql) }

    /// Writes the current state into a new immutable base; existing readers retain their snapshot.
    pub fn compact(&self) -> std::io::Result<()> { self.store.compact() }
}

/// An immutable view of the database at the instant `Database::read` was called.
pub struct ReadSnapshot { snapshot: Arc<Overlay> }
impl ReadSnapshot {
    pub fn query(&self, sparql: &str) -> Result<QueryResult, QueryError> {
        exec::query(&*self.snapshot, sparql)
    }
}
