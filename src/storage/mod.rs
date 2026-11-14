//! Storage API (owner: Alex-Chi, task #1). Executor (task #4) and optimizer (task #3) code only against this.
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
    /// Exact number of triples matching the prefix (from block index / aggregated counts, no full scan).
    fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError>;
    /// Seek support for leapfrog joins: smallest value >= `target` in key position `prefix.len()` under `prefix`.
    fn seek(
        &self,
        perm: Perm,
        prefix: &[TermId],
        target: TermId,
    ) -> Result<Option<TermId>, StorageError>;
}

mod memory;
pub use memory::MemorySnapshot;

#[cfg(unix)]
pub mod disk_index;

#[cfg(unix)]
mod block_codec;

#[cfg(unix)]
mod term_block_codec;

#[cfg(unix)]
mod lazy_dictionary;

#[cfg(unix)]
mod external_sort;

#[cfg(unix)]
pub mod external_build;

#[cfg(unix)]
mod disk_snapshot;
#[cfg(unix)]
pub use disk_snapshot::{DiskBase, DiskSnapshot, build_disk_snapshot, write_disk_snapshot};
