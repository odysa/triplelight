//! TripleLight: an embedded analytical RDF database. See DESIGN.md.
//!
//! This crate is the public face of the workspace: it re-exports the component crates under stable paths and adds
//! the embedded database API.
pub use triplelight_core::{model, ntriples, rdf, turtle};
pub use triplelight_exec as exec;
pub use triplelight_plan as opt;
pub use triplelight_sparql as sparql;
pub mod shacl;

/// Storage traits and implementations.
pub mod storage {
    pub use triplelight_core::storage::*;
    pub use triplelight_storage::*;
}

/// The durable store (WAL, overlay, compaction), with SPARQL Update wired to the executor.
pub mod store {
    pub use triplelight_storage::store::*;

    /// SPARQL Update on a [`Store`]: the change set is computed by the executor inside the store's writer
    /// serialization (see `Store::update_dataset_with`), and named-graph steps are durable.
    pub trait StoreUpdate {
        /// Executes a SPARQL 1.1 Update request atomically and durably; returns the number of triples changed.
        fn update(&self, text: &str) -> Result<usize, crate::exec::QueryError>;
    }

    impl StoreUpdate for Store {
        fn update(&self, text: &str) -> Result<usize, crate::exec::QueryError> {
            store_update_with(self, text, dataset_update)
        }
    }

    pub(crate) type DatasetUpdate = (Option<Overlay>, Vec<crate::model::DatasetChange>, usize);

    pub(crate) fn store_update_with(
        store: &Store,
        text: &str,
        compute: impl Fn(&Overlay, &str) -> Result<DatasetUpdate, crate::exec::QueryError>,
    ) -> Result<usize, crate::exec::QueryError> {
        // Return the sum of the operations' counts. The WAL records their net effect.
        let changed = std::cell::Cell::new(0);
        store.update_dataset_with(text, &|snapshot, text| {
            let (_, changes, count) = compute(snapshot, text)?;
            changed.set(count);
            Ok(changes)
        }, &storage_error)?;
        Ok(changed.get())
    }

    /// A storage failure as a query error. A graph-state error (`CREATE` of an existing graph, `DROP`/`CLEAR` of an
    /// absent one) is the request's fault, so it keeps its own kind instead of becoming a storage error.
    pub fn storage_error(e: std::io::Error) -> crate::exec::QueryError {
        use crate::exec::{QueryError, QueryErrorKind};
        let kind = if e.get_ref().is_some_and(|inner| inner.is::<GraphStateError>()) { QueryErrorKind::GraphState } else { QueryErrorKind::Storage };
        QueryError::new(kind, e.to_string())
    }

    /// Executes the operations of a SPARQL 1.1 Update request in order on in-memory copies of `snap` (task #36):
    /// each operation's WHERE clause sees the effects of the operations before it. Returns the resulting snapshot,
    /// the concatenated dataset changes (to commit atomically) and the number of triples that changed.
    pub fn dataset_update(snap: &Overlay, text: &str)
        -> Result<DatasetUpdate, crate::exec::QueryError> {
        let operations = crate::exec::parse_update_operations(text)?;
        dataset_update_operations(snap, &operations)
    }

    pub(crate) fn dataset_update_operations(snap: &Overlay, operations: &[triplelight_sparql::GraphUpdateOperation])
        -> Result<DatasetUpdate, crate::exec::QueryError> {
        let storage = storage_error;
        let mut owned: Option<Overlay> = None;
        let mut all = Vec::new();
        let mut changed = 0;
        for op in operations {
            let cur = owned.as_ref().unwrap_or(snap);
            let changes = crate::exec::operation_changes(cur, cur, op)?;
            if changes.is_empty() { continue }
            let (next, n) = cur.with_dataset_changes(&changes).map_err(storage)?;
            owned = Some(next);
            changed += n;
            all.extend(changes);
        }
        Ok((owned, all, changed))
    }

    /// The inserts of a loaded N-Quads or TriG document, as one dataset change list (task #36). Blank-node labels are
    /// scoped to the document and shared across its graphs, so `_:b` in two loads is two nodes. A quad whose graph
    /// name is a blank node is refused as unsupported.
    pub fn quad_load_changes(quads: Vec<crate::rdf::Quad>) -> Result<Vec<crate::model::DatasetChange>, crate::exec::QueryError> {
        use crate::exec::{QueryError, QueryErrorKind};
        use crate::model::{DatasetChange, GraphTarget};
        use crate::rdf::{BlankNode, GraphName, NamedOrBlankNode, Triple};
        let mut scope: std::collections::HashMap<BlankNode, BlankNode> = std::collections::HashMap::new();
        let mut fresh = |b: &BlankNode| scope.entry(b.clone()).or_default().clone();
        quads.into_iter().map(|q| {
            let graph = match q.graph_name {
                GraphName::DefaultGraph => GraphTarget::Default,
                GraphName::NamedNode(n) => GraphTarget::Named(n),
                GraphName::BlankNode(_) => return Err(QueryError::new(QueryErrorKind::Unsupported, "blank-node graph names are not supported")),
            };
            let subject = match q.subject { NamedOrBlankNode::BlankNode(b) => NamedOrBlankNode::BlankNode(fresh(&b)), s => s };
            // Blank nodes inside RDF 1.2 triple terms are scoped to the document too.
            let object = crate::exec::map_blank_nodes(q.object, &mut fresh);
            Ok(DatasetChange::Quad(graph, Change::Insert, Triple::new(subject, q.predicate, object)))
        }).collect()
    }
}

#[cfg(unix)]
pub mod embedded;

/// The async embedded API (feature `async`): every call runs on a worker and is awaited on any executor.
#[cfg(feature = "async")]
pub mod async_embedded;
