//! The storage interface the query side is written against (snapshots and the term dictionary).
use crate::model::{Perm, TermId, TripleBatch};
use oxrdf::{Term, TermRef};

pub type StorageError = std::io::Error;

pub trait Dictionary {
    /// Return the existing ID, or None if absent; storage failures are errors.
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError>;
    fn resolve(&self, id: TermId) -> Result<Term, StorageError>;
    /// Borrow a term only for the duration of the callback, allowing cached disk blocks.
    fn with_term(&self, id: TermId, f: &mut dyn FnMut(TermRef<'_>)) -> Result<(), StorageError>;
    /// Callbacks carry input positions and may be delivered in any order. On error,
    /// callers must discard partial output already produced by the callback.
    fn with_terms(
        &self,
        ids: &[TermId],
        f: &mut dyn FnMut(usize, TermRef<'_>),
    ) -> Result<(), StorageError> {
        for (position, &id) in ids.iter().enumerate() {
            self.with_term(id, &mut |term| f(position, term))?;
        }
        Ok(())
    }
}

/// A consistent read view (snapshot) of the store.
pub trait Snapshot: Dictionary + Send + Sync {
    /// Sorted scan of `perm` where the first `prefix.len()` key components (0..=3) equal `prefix`.
    /// Yields batches of at most ~4096 rows, sorted by the full key.
    fn scan<'a>(
        &'a self,
        perm: Perm,
        prefix: &[TermId],
    ) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a>;
    /// Full ordered scan intended for one-pass statistics; backends may bypass their query cache.
    fn scan_for_stats(&self) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + '_> {
        self.scan(Perm::Spo, &[])
    }
    /// Exact number of triples matching the prefix (from block index / aggregated counts, no full scan).
    fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError>;
    /// Seek support for leapfrog joins: smallest value >= `target` in key position `prefix.len()` under `prefix`.
    fn seek(
        &self,
        perm: Perm,
        prefix: &[TermId],
        target: TermId,
    ) -> Result<Option<TermId>, StorageError>;
    /// Zero-copy view of the rows `scan` would yield (key order of `perm`), for stores that keep the permutation as one
    /// in-memory sorted array. Lets an index nested-loop join probe without allocating a batch per input row.
    fn matching_rows(&self, _perm: Perm, _prefix: &[TermId]) -> Option<&[[TermId; 3]]> { None }
}


/// An RDF dataset read view: the default graph plus a catalog of named graphs, all sharing one dictionary and one
/// consistent snapshot (provisional query-side contract for named graphs, task #36).
///
/// An absent graph (`graph` returns `Ok(None)`) differs from an empty one (`Ok(Some(view))` with no triples); the
/// catalog lists every named graph, empty ones included. Graph names are dictionary IDs of IRIs.
pub trait DatasetView: Send + Sync {
    /// The stored default graph (distinct from every named graph).
    fn default_graph(&self) -> &dyn Snapshot;
    /// The named graph `name`, or `None` if the catalog does not contain it.
    fn graph(&self, name: TermId) -> Result<Option<&dyn Snapshot>, StorageError>;
    /// Every named graph in the catalog, in ascending ID order, empty graphs included.
    fn graph_names(&self) -> Result<Vec<TermId>, StorageError>;
    /// Whether some blank node occurs in more than one of `graphs`. A FROM default graph is the RDF merge of its
    /// sources, which standardizes blank nodes apart; a plain union over shared IDs is only that merge when no blank
    /// node is shared. `None` means the store cannot tell cheaply, and the evaluator then refuses such a FROM.
    fn shares_blank_nodes(&self, _graphs: &[TermId]) -> Result<Option<bool>, StorageError> { Ok(None) }
}
