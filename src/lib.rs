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
            // Report the sum of the operations' changed triples, as the in-memory database does (not the net effect
            // of the concatenated list, which is what the WAL records).
            let changed = std::cell::Cell::new(0);
            self.update_with(text, &|snap, text| {
                let (_, changes, n) = dataset_update(snap, text)?;
                changed.set(n);
                default_graph_changes(&changes)
            }, &|e| QueryError::new(QueryErrorKind::Storage, e.to_string()))?;
            Ok(changed.get())
        }
    }

    /// Executes the operations of a SPARQL 1.1 Update request in order on in-memory copies of `snap` (task #36):
    /// each operation's WHERE clause sees the effects of the operations before it. Returns the resulting snapshot,
    /// the concatenated dataset changes (to commit atomically) and the number of triples that changed.
    pub fn dataset_update(snap: &Overlay, text: &str)
        -> Result<(Option<Overlay>, Vec<crate::model::DatasetChange>, usize), crate::exec::QueryError> {
        use crate::exec::{QueryError, QueryErrorKind};
        let storage = |e: std::io::Error| QueryError::new(QueryErrorKind::Storage, e.to_string());
        let mut owned: Option<Overlay> = None;
        let mut all = Vec::new();
        let mut changed = 0;
        for op in crate::exec::parse_update_operations(text)? {
            let cur = owned.as_ref().unwrap_or(snap);
            let changes = crate::exec::operation_changes(cur, cur, &op)?;
            if changes.is_empty() { continue }
            let (next, n) = cur.with_dataset_changes(&changes).map_err(storage)?;
            owned = Some(next);
            changed += n;
            all.extend(changes);
        }
        Ok((owned, all, changed))
    }

    /// The default-graph triple changes of `changes`, for a store that does not yet record named graphs durably.
    /// Any named-graph step is refused, rather than applied in memory and lost on restart.
    pub fn default_graph_changes(changes: &[crate::model::DatasetChange])
        -> Result<Vec<(Change, oxrdf::Triple)>, crate::exec::QueryError> {
        use crate::exec::{QueryError, QueryErrorKind};
        use crate::model::{DatasetChange, GraphTarget};
        changes.iter().map(|c| match c {
            DatasetChange::Quad(GraphTarget::Default, change, t) => Ok((*change, t.clone())),
            _ => Err(QueryError::new(QueryErrorKind::Unsupported,
                "named graphs are not supported on a persistent store yet (the in-memory database supports them)")),
        }).collect()
    }
}

#[cfg(unix)]
pub mod embedded;
