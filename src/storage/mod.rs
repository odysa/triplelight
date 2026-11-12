//! Storage API (owner: Alex-Chi, task #1). Executor (task #4) and optimizer (task #3) code only against this.
use crate::model::{Perm, TermId, TripleBatch};
use oxrdf::Term;

pub trait Dictionary {
    /// Existing ID for a term, or None if the term never occurs (lets the planner return empty early).
    fn lookup(&self, term: &Term) -> Option<TermId>;
    fn resolve(&self, id: TermId) -> Term;
}

/// A consistent read view (snapshot) of the store.
pub trait Snapshot: Dictionary + Send + Sync {
    /// Sorted scan of `perm` where the first `prefix.len()` key components (0..=3) equal `prefix`.
    /// Yields batches of at most ~4096 rows, sorted by the full key.
    fn scan<'a>(&'a self, perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = TripleBatch> + 'a>;
    /// Exact number of triples matching the prefix (from block index / aggregated counts, no full scan).
    fn count(&self, perm: Perm, prefix: &[TermId]) -> u64;
    /// Seek support for leapfrog joins: smallest value >= `target` in key position `prefix.len()` under `prefix`.
    fn seek(&self, perm: Perm, prefix: &[TermId], target: TermId) -> Option<TermId>;
}

mod memory;
pub use memory::MemorySnapshot;
