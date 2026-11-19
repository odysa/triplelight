//! TripleLight: an embedded analytical RDF database. See DESIGN.md.
//!
//! This crate is the public face of the workspace: it re-exports the component crates under stable paths and adds
//! the embedded database API.
pub use triplelight_core::{model, rdf};
pub use triplelight_exec as exec;
pub use triplelight_plan as opt;
pub use triplelight_sparql as sparql;

/// Storage traits and implementations.
pub mod storage {
    pub use triplelight_core::storage::*;
    pub use triplelight_storage::*;
}

/// The durable store (WAL, overlay, compaction), with SPARQL Update wired to the executor.
pub mod store {
    pub use triplelight_storage::store::*;

    /// SPARQL Update on a [`Store`]: the change set is computed by the executor inside the store's writer
    /// serialization (see `Store::update_with`).
    pub trait StoreUpdate {
        /// Executes a SPARQL 1.1 Update request atomically and durably; returns the number of triples changed.
        fn update(&self, text: &str) -> Result<usize, crate::exec::QueryError>;
    }

    impl StoreUpdate for Store {
        fn update(&self, text: &str) -> Result<usize, crate::exec::QueryError> {
            use crate::exec::{QueryError, QueryErrorKind};
            self.update_with(text, &|snap, text| crate::exec::update_changes(snap, text),
                &|e| QueryError::new(QueryErrorKind::Storage, e.to_string()))
        }
    }
}

#[cfg(unix)]
pub mod embedded;
