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
    /// In pattern order, so the scanner can fill the existing BGP row slots.
    pub columns: Vec<(TermId, usize)>,
    /// Sorted, unique predicate IDs for the catalog lookup.
    pub predicates: Vec<TermId>,
}

/// Recognizes the narrow shape whose table expansion has no equality joins:
/// one subject variable, distinct fixed predicates and distinct object variables.
/// Constants, repeated object variables and mixed BGPs are left to the triple path.
pub(super) fn recognize_star(patterns: &[[Slot; 3]], all_unbound: bool) -> Option<Star> {
    if !all_unbound || patterns.len() < 2 {
        return None;
    }
    let subject_slot = match patterns[0][0] {
        Slot::Var(slot) => slot,
        Slot::Const(_) => return None,
    };
    let mut predicates = Vec::with_capacity(patterns.len());
    let mut columns = Vec::with_capacity(patterns.len());
    let mut objects = HashSet::with_capacity(patterns.len());
    for pattern in patterns {
        if !matches!(pattern[0], Slot::Var(slot) if slot == subject_slot) {
            return None;
        }
        let predicate = match pattern[1] {
            Slot::Const(Some(id)) => id,
            _ => return None,
        };
        let object = match pattern[2] {
            Slot::Var(slot) if slot != subject_slot && objects.insert(slot) => slot,
            _ => return None,
        };
        predicates.push(predicate);
        columns.push((predicate, object));
    }
    predicates.sort_unstable();
    predicates.dedup();
    (predicates.len() == patterns.len()).then_some(Star {
        subject_slot,
        columns,
        predicates,
    })
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
    fn recognizes_only_the_initial_safe_star_shape() {
        let patterns = star();
        let got = recognize_star(&patterns, true).unwrap();
        assert_eq!(got.subject_slot, 0);
        assert_eq!(got.predicates, vec![TermId(10), TermId(20)]);
        assert_eq!(got.columns, vec![(TermId(10), 1), (TermId(20), 2)]);
        assert!(recognize_star(&patterns, false).is_none());
        assert!(recognize_star(&patterns[..1], true).is_none());
        let mut repeated_object = patterns;
        repeated_object[1][2] = Slot::Var(1);
        assert!(recognize_star(&repeated_object, true).is_none());
        let mut variable_predicate = patterns;
        variable_predicate[1][1] = Slot::Var(3);
        assert!(recognize_star(&variable_predicate, true).is_none());
        let mut duplicate_predicate = patterns;
        duplicate_predicate[1][1] = Slot::Const(Some(TermId(10)));
        assert!(recognize_star(&duplicate_predicate, true).is_none());
        let mut other_subject = patterns;
        other_subject[1][0] = Slot::Var(4);
        assert!(recognize_star(&other_subject, true).is_none());
        let mut object_constant = patterns;
        object_constant[1][2] = Slot::Const(Some(TermId(30)));
        assert!(recognize_star(&object_constant, true).is_none());
    }

    struct Mock {
        ids: Vec<TableId>,
        metas: HashMap<TableId, TableMeta>,
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
        let spec = recognize_star(&star(), true).unwrap();
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
}
