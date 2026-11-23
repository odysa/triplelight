//! TripleLight storage: the in-memory and on-disk triple stores, the term dictionary, and the durable store (WAL,
//! overlay, compaction) in `store`.
// Paths used throughout this crate (`crate::model`, `crate::storage::Snapshot`, ...).
pub(crate) use triplelight_core::model;
pub use triplelight_core::storage::{Dictionary, Snapshot, StorageError};
pub(crate) mod storage { pub use crate::*; }
#[cfg(test)]
pub(crate) use triplelight_exec as exec;

pub mod store;

#[cfg(all(test, feature = "rdf-12"))]
mod rdf12_tests;

mod memory;
pub use memory::MemorySnapshot;

#[cfg(unix)]
pub mod disk_index;

#[cfg(unix)]
mod base_format;

#[cfg(unix)]
mod block_codec;

#[cfg(unix)]
mod term_block_codec;

#[cfg(unix)]
mod lazy_dictionary;

#[cfg(unix)]
mod external_sort;

#[cfg(unix)]
mod fixed_sort;

#[cfg(unix)]
pub mod external_build;
#[cfg(all(test, unix))]
mod external_build_tests;

#[cfg(unix)]
mod derived_order;
#[cfg(unix)]
pub use derived_order::trace as derived_trace;

#[cfg(unix)]
mod disk_snapshot;
#[cfg(unix)]
pub use base_format::{ALL_ORDERS, REQUIRED_ORDERS};
#[cfg(unix)]
pub use disk_snapshot::{DiskBase, DiskGraph, DiskSnapshot, build_disk_snapshot, build_disk_snapshot_with_workers, write_disk_dataset, write_disk_snapshot};
