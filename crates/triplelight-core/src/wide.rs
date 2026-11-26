//! Wide tables (#140): an optional per-entity column layout over an immutable view, as an extra access path.
//!
//! Subjects whose property sets are alike are stored together as one table: a sorted subject column and one column
//! per predicate holding that subject's objects. Every triple stays in the permutation indexes as well, so a plan
//! that ignores the tables is always valid. A star of triple patterns on one subject variable with fixed predicates
//! can be answered as the union of every table that has all its predicates as columns, plus the ordinary BGP over
//! the indexes restricted to subjects that are in none of those tables (`member`). A subject is in at most one
//! table, so that union neither loses nor repeats a solution.
//!
//! Bag semantics: a subject with `k_i` values for the `i`-th requested predicate yields `prod k_i` rows, one per
//! value combination, in ascending order of the values; a subject missing any requested predicate yields none.
//! This is exactly what the BGP over the indexes produces for that subject, so the two can be compared as bags.
use crate::model::TermId;
/// Same type as `crate::storage::StorageError`, named directly so this module does not depend on `storage`.
type StorageError = std::io::Error;

/// A table of one catalog.
pub type TableId = u32;

/// One predicate column of a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnMeta {
    pub predicate: TermId,
    /// Subjects with at least one value for this predicate.
    pub non_null: u64,
    /// Values (triples) in the column; `values > non_null` means some subject has several.
    pub values: u64,
    /// Bytes the column occupies in this catalog's representation.
    pub bytes: u64,
}

/// What a planner needs to cost a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableMeta {
    pub subjects: u64,
    /// Sorted by predicate ID.
    pub columns: Vec<ColumnMeta>,
    /// Bytes of the subject column plus the membership test.
    pub subject_bytes: u64,
}

impl TableMeta {
    pub fn column(&self, predicate: TermId) -> Option<&ColumnMeta> {
        self.columns.binary_search_by_key(&predicate, |c| c.predicate).ok().map(|i| &self.columns[i])
    }
}

/// Expanded rows of one table scan: `subjects[r]` and `values[c][r]` (column `c` in the order the scan asked for).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TableBatch {
    pub subjects: Vec<TermId>,
    pub values: Vec<Vec<TermId>>,
}

impl TableBatch {
    pub fn len(&self) -> usize { self.subjects.len() }
    pub fn is_empty(&self) -> bool { self.subjects.is_empty() }
}

/// What a scan did, for EXPLAIN and cost calibration.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ScanCounters {
    pub subjects_scanned: u64,
    pub values_read: u64,
    pub rows_emitted: u64,
    pub bytes_read: u64,
}

/// A running table scan.
pub trait TableScan {
    /// The next batch of at most `TABLE_BATCH_ROWS` rows (a subject's rows are never split across batches unless
    /// it alone exceeds the limit); `None` when done.
    fn next_batch(&mut self) -> Option<Result<TableBatch, StorageError>>;
    fn counters(&self) -> ScanCounters;
}

/// Target rows per `TableBatch`.
pub const TABLE_BATCH_ROWS: usize = 1024;

/// The tables of one immutable view.
pub trait TableCatalog: Send + Sync {
    /// Every table whose columns include all of `predicates` (duplicates allowed), ascending.
    fn tables_with(&self, predicates: &[TermId]) -> Vec<TableId>;
    fn meta(&self, table: TableId) -> &TableMeta;
    /// Whether `subject` is stored in `table` (constant time).
    fn member(&self, table: TableId, subject: TermId) -> bool;
    /// Whether every subject of the view that has `predicate` is in some table (exact, counted at build time). When
    /// it holds for any fixed predicate of a star, the star's residual over the indexes is empty and can be skipped,
    /// provided both of these hold (they do for `WideTables`): a table has a column for every predicate any of its
    /// subjects has (so a subject matching the star sits in a table that `tables_with(star)` returns), and the plan
    /// scans every table `tables_with(star)` returns. A catalog that drops columns must return `false` here.
    fn covers_all_subjects(&self, _predicate: TermId) -> bool { false }
    /// Rows of `table` for `predicates` (each must be a column; repeats allowed), subjects ascending. `ranges[i]`, if
    /// given, keeps only values `v` of `predicates[i]` with `lo <= v < hi` (by ID) before expansion; the caller is
    /// responsible for the ranges being exact for its filter.
    fn scan<'a>(&'a self, table: TableId, predicates: &[TermId], ranges: &[Option<(TermId, TermId)>]) -> Box<dyn TableScan + 'a>;
}
