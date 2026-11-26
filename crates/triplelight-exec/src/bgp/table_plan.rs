//! Conservative recognition and costing for a wide-table access path.
//!
//! The first physical implementation accepts only a complete, all-unbound star.
//! A rejected shape is evaluated by the existing triple-index BGP executor.
//! Automatic selection waits for calibrated residual and scan costs. The
//! physical path is available through an explicit diagnostic switch.

use crate::model::TermId;
use crate::pattern::Slot;
use std::collections::HashSet;
use triplelight_core::wide::{TableCatalog, TableId};

#[derive(Clone, Debug)]
pub(super) struct Star {
    pub subject_slot: usize,
    /// Indices of the patterns answered by the table, in BGP order.
    pub pattern_indices: Vec<usize>,
    /// In pattern order, so the scanner can fill the existing BGP row slots.
    pub columns: Vec<(TermId, Slot)>,
    /// Sorted, unique predicate IDs for the catalog lookup.
    pub predicates: Vec<TermId>,
}

/// Finds stars whose table expansion has no equality joins: one subject
/// variable, distinct fixed predicates and distinct object variables or
/// constants. Largest first; the caller tries each until its catalog has a
/// matching table. Other patterns remain ordinary index joins.
pub(super) fn recognize_stars(patterns: &[[Slot; 3]], all_unbound: bool) -> Vec<Star> {
    if !all_unbound || patterns.len() < 2 { return Vec::new() }
    let mut stars = Vec::new();
    let mut subjects = HashSet::new();
    for pattern in patterns {
        let Slot::Var(subject_slot) = pattern[0] else { continue };
        if !subjects.insert(subject_slot) { continue }
        let mut predicates = Vec::new();
        let mut columns = Vec::new();
        let mut pattern_indices = Vec::new();
        let mut objects = HashSet::new();
        for (i, pattern) in patterns.iter().enumerate() {
            if !matches!(pattern[0], Slot::Var(slot) if slot == subject_slot) { continue }
            let Slot::Const(Some(predicate)) = pattern[1] else { continue };
            if predicates.contains(&predicate) { continue }
            match pattern[2] {
                Slot::Var(slot) if slot != subject_slot && objects.insert(slot) => {},
                Slot::Const(Some(id)) if id.0 < u64::MAX => {},
                _ => continue,
            }
            predicates.push(predicate);
            columns.push((predicate, pattern[2]));
            pattern_indices.push(i);
        }
        if pattern_indices.len() < 2 { continue }
        predicates.sort_unstable();
        stars.push(Star { subject_slot, pattern_indices, columns, predicates });
    }
    stars.sort_by_key(|star| std::cmp::Reverse(star.pattern_indices.len()));
    stars
}

/// The first, opt-in cost rule for complete, reorderable two-pattern stars with a canonical
/// object interval. A table visits every subject, while an index can start from
/// a selective object range. The seven frozen BI cases motivate a provisional
/// 4% guard; it must be checked against the actual table metadata. Embedded and
/// longer stars, fixed objects and residuals await their own calibration.
pub(super) fn select_auto(
    catalog: &dyn TableCatalog,
    star: &Star,
    tables: &[TableId],
    index_start_rows: u64,
    intervals: &[Option<(TermId, TermId)>],
) -> bool {
    if tables.is_empty() || star.pattern_indices.len() != 2
        || !star.columns.iter().all(|(_, object)| matches!(object, Slot::Var(_))) {
        return false;
    }
    if !star.predicates.iter().any(|&p| catalog.covers_all_subjects(p)) {
        return false;
    }
    if !intervals.iter().any(Option::is_some) || index_start_rows == 0 { return false }
    let subjects = tables.iter().fold(0u64, |n, &table| n.saturating_add(catalog.meta(table).subjects));
    if subjects == 0 { return false }
    (index_start_rows as f64) >= 0.04 * subjects as f64
}

/// A residual estimate must include the indexed BGP over subjects not assigned
/// to the selected tables. Passing `None` forces the ordinary index plan.
#[derive(Clone, Copy, Debug)]
pub(super) struct ResidualStats {
    pub indexed_ns: f64,
    pub membership_probes: u64,
}

/// Coefficients are measured on the target machine and kept out of query text.
/// `bytes_per_ns` is effective sequential throughput, not a hardware ceiling.
#[derive(Clone, Copy, Debug)]
pub(super) struct CostCalibration {
    pub bytes_per_ns: f64,
    pub subject_ns: f64,
    pub value_ns: f64,
    pub output_row_ns: f64,
    pub membership_probe_ns: f64,
    /// Require a substantial margin while table estimates are immature.
    pub max_table_to_index_ratio: f64,
}

impl CostCalibration {
    fn valid(self) -> bool {
        self.bytes_per_ns.is_finite()
            && self.bytes_per_ns > 0.0
            && [
                self.subject_ns,
                self.value_ns,
                self.output_row_ns,
                self.membership_probe_ns,
            ]
            .into_iter()
            .all(|v| v.is_finite() && v >= 0.0)
            && self.max_table_to_index_ratio.is_finite()
            && (0.0..1.0).contains(&self.max_table_to_index_ratio)
    }
}

#[derive(Clone, Debug)]
pub(super) struct TableChoice {
    pub tables: Vec<TableId>,
    pub estimated_table_ns: f64,
    pub estimated_index_ns: f64,
    pub estimated_rows: f64,
}

/// Compares the *complete* table+residual path with the already chosen index
/// path. It never treats a table's matching schema as a completeness proof.
pub(super) fn choose(
    catalog: &dyn TableCatalog,
    star: &Star,
    index_ns: f64,
    residual: Option<ResidualStats>,
    calibration: CostCalibration,
) -> Option<TableChoice> {
    if !calibration.valid() || !index_ns.is_finite() || index_ns <= 0.0 {
        return None;
    }
    let tables = catalog.tables_with(&star.predicates);
    if tables.is_empty()
        || tables
            .iter()
            .enumerate()
            .any(|(i, id)| tables[..i].contains(id))
    {
        return None;
    }
    let residual = residual?;
    if !residual.indexed_ns.is_finite() || residual.indexed_ns < 0.0 {
        return None;
    }
    let mut table_ns =
        residual.indexed_ns + residual.membership_probes as f64 * calibration.membership_probe_ns;
    let mut rows = 0.0;
    for table in &tables {
        let meta = catalog.meta(*table);
        if meta.subjects == 0 {
            continue;
        }
        let mut bytes = meta.subject_bytes as f64;
        let mut values = 0.0;
        // The average-fanout product is a cost estimate, not an answer count.
        // Incorrect correlation changes plan choice only; the scan is exact.
        let mut table_rows = meta.subjects as f64;
        for predicate in &star.predicates {
            let column = meta.column(*predicate)?;
            if column.non_null > meta.subjects || column.values < column.non_null {
                return None;
            }
            bytes += column.bytes as f64;
            values += column.values as f64;
            table_rows *= column.values as f64 / meta.subjects as f64;
        }
        rows += table_rows;
        table_ns += bytes / calibration.bytes_per_ns
            + meta.subjects as f64 * calibration.subject_ns
            + values * calibration.value_ns
            + table_rows * calibration.output_row_ns;
    }
    if !table_ns.is_finite() || !rows.is_finite() {
        return None;
    }
    (table_ns <= index_ns * calibration.max_table_to_index_ratio).then_some(TableChoice {
        tables,
        estimated_table_ns: table_ns,
        estimated_index_ns: index_ns,
        estimated_rows: rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use triplelight_core::storage::StorageError;
    use triplelight_core::wide::{ColumnMeta, ScanCounters, TableBatch, TableMeta, TableScan};

    fn star() -> [[Slot; 3]; 2] {
        [
            [Slot::Var(0), Slot::Const(Some(TermId(10))), Slot::Var(1)],
            [Slot::Var(0), Slot::Const(Some(TermId(20))), Slot::Var(2)],
        ]
    }

    #[test]
    fn recognizes_star_with_constants_inside_a_larger_bgp() {
        let patterns = star();
        let got = recognize_stars(&patterns, true).remove(0);
        assert_eq!(got.subject_slot, 0);
        assert_eq!(got.pattern_indices, vec![0, 1]);
        assert_eq!(got.predicates, vec![TermId(10), TermId(20)]);
        assert_eq!(got.columns, vec![(TermId(10), Slot::Var(1)), (TermId(20), Slot::Var(2))]);
        assert!(recognize_stars(&patterns, false).is_empty());
        assert!(recognize_stars(&patterns[..1], true).is_empty());
        let mut repeated_object = patterns;
        repeated_object[1][2] = Slot::Var(1);
        assert!(recognize_stars(&repeated_object, true).is_empty());
        let mut variable_predicate = patterns;
        variable_predicate[1][1] = Slot::Var(3);
        assert!(recognize_stars(&variable_predicate, true).is_empty());
        let mut duplicate_predicate = patterns;
        duplicate_predicate[1][1] = Slot::Const(Some(TermId(10)));
        assert!(recognize_stars(&duplicate_predicate, true).is_empty());
        let mut other_subject = patterns;
        other_subject[1][0] = Slot::Var(4);
        assert!(recognize_stars(&other_subject, true).is_empty());
        let mut object_constant = patterns;
        object_constant[1][2] = Slot::Const(Some(TermId(30)));
        assert_eq!(
            recognize_stars(&object_constant, true).remove(0).columns[1],
            (TermId(20), Slot::Const(Some(TermId(30))))
        );
        let mixed = [patterns[0], [Slot::Var(3), Slot::Const(Some(TermId(40))), Slot::Var(1)], patterns[1]];
        assert_eq!(recognize_stars(&mixed, true).remove(0).pattern_indices, vec![0, 2]);
    }

    struct Mock {
        ids: Vec<TableId>,
        metas: HashMap<TableId, TableMeta>,
        covered: bool,
    }

    struct EmptyScan;

    impl TableScan for EmptyScan {
        fn next_batch(&mut self) -> Option<Result<TableBatch, StorageError>> {
            None
        }
        fn counters(&self) -> ScanCounters {
            ScanCounters::default()
        }
    }

    impl TableCatalog for Mock {
        fn tables_with(&self, _: &[TermId]) -> Vec<TableId> {
            self.ids.clone()
        }
        fn meta(&self, table: TableId) -> &TableMeta {
            &self.metas[&table]
        }
        fn member(&self, _: TableId, _: TermId) -> bool {
            false
        }
        fn covers_all_subjects(&self, _: TermId) -> bool { self.covered }
        fn scan<'a>(
            &'a self,
            _: TableId,
            _: &[TermId],
            _: &[Option<(TermId, TermId)>],
        ) -> Box<dyn TableScan + 'a> {
            Box::new(EmptyScan)
        }
    }

    fn calibration() -> CostCalibration {
        CostCalibration {
            bytes_per_ns: 8.0,
            subject_ns: 2.0,
            value_ns: 1.0,
            output_row_ns: 1.0,
            membership_probe_ns: 1.0,
            max_table_to_index_ratio: 0.5,
        }
    }

    #[test]
    fn costing_includes_all_tables_and_the_residual() {
        let spec = recognize_stars(&star(), true).remove(0);
        let one_table = TableMeta {
            subjects: 100,
            subject_bytes: 800,
            columns: vec![
                ColumnMeta {
                    predicate: TermId(10),
                    non_null: 100,
                    values: 200,
                    bytes: 800,
                },
                ColumnMeta {
                    predicate: TermId(20),
                    non_null: 100,
                    values: 100,
                    bytes: 800,
                },
            ],
        };
        let mock = Mock {
            ids: vec![1, 2],
            metas: HashMap::from([(1, one_table.clone()), (2, one_table)]),
            covered: false,
        };
        let residual = Some(ResidualStats {
            indexed_ns: 100.0,
            membership_probes: 50,
        });
        let choice = choose(&mock, &spec, 10_000.0, residual, calibration()).unwrap();
        assert_eq!(choice.tables, vec![1, 2]);
        assert_eq!(choice.estimated_rows, 400.0);
        // Each table: 300 bytes/ns + 200 subject + 300 values + 200 output.
        assert_eq!(choice.estimated_table_ns, 2_000.0 + 100.0 + 50.0);
        let expensive_residual = Some(ResidualStats {
            indexed_ns: 5_000.0,
            membership_probes: 50,
        });
        assert!(choose(&mock, &spec, 10_000.0, expensive_residual, calibration()).is_none());
        assert!(choose(&mock, &spec, 10_000.0, None, calibration()).is_none());
    }

    #[test]
    fn auto_choice_uses_range_work_and_requires_complete_coverage() {
        let star = recognize_stars(&star(), true).remove(0);
        let mut mock = Mock {
            ids: vec![1],
            metas: HashMap::from([(1, TableMeta { subjects: 2_850_000, subject_bytes: 0, columns: Vec::new() })]),
            covered: true,
        };
        let intervals = [None, Some((TermId(100), TermId(200)))];
        assert!(!select_auto(&mock, &star, &[1], 75_963, &intervals));
        assert!(select_auto(&mock, &star, &[1], 168_210, &intervals));
        assert!(select_auto(&mock, &star, &[1], 2_850_000, &intervals));
        assert!(!select_auto(&mock, &star, &[1], 2_850_000, &[None, None]));
        mock.covered = false;
        assert!(!select_auto(&mock, &star, &[1], 168_210, &intervals));
        let mut fixed = star.clone();
        fixed.columns[0].1 = Slot::Const(Some(TermId(50)));
        mock.covered = true;
        assert!(!select_auto(&mock, &fixed, &[1], 168_210, &intervals));
    }
}
