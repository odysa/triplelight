//! GROUP BY over an input that may not fit in the memory budget (#111): the input rows are hash-partitioned on the
//! group keys into run files under the budget's scratch directory (`spill::Partitions`), the in-memory input is
//! dropped, and each partition is grouped on its own, so one partition's rows and groups are in memory at a time. A
//! key lives in exactly one partition, so every group is complete. Groups come out in first-seen order, as from the
//! in-memory path: each is tagged with the input position of its first row and the partitions' groups are merged on it.
use super::super::{phase, spill, storage_failure, Ctx, Row};
use crate::model::TermId;
use crate::storage::Snapshot;
use std::collections::HashMap;

impl<S: Snapshot + ?Sized> Ctx<'_, S> {
    /// Where to spill a blocking input of `rows` rows (`row_bytes` each), and into how many partitions: `None` keeps it
    /// in memory. The input is resident but not held (`Ctx::eval` only checks that it fits), and the operator's working
    /// set holds the rows once more (in groups or a table), so it spills when the budget has a scratch directory and
    /// twice the input does not fit what is free.
    pub(crate) fn spill_plan(&self, rows: usize) -> Option<(std::path::PathBuf, usize)> {
        let bytes = rows as u64 * self.row_bytes();
        let budget = self.budget?;
        let dir = budget.scratch_dir()?;
        if let Some(n) = budget.spill_rows() { return (rows > n).then(|| (dir.to_path_buf(), 4)) }
        (bytes.saturating_mul(2) > budget.available()).then(|| (dir.to_path_buf(), spill::partitions_for(bytes, budget.available())))
    }

    /// Writes a GROUP BY input of `n_rows` rows (each `vars.len()` IDs, keys at `kidx`) to disk by key partition when it
    /// does not fit (`spill_plan`); `None` keeps it in memory. The caller drops its in-memory rows, then calls
    /// `group_partitions`.
    pub(crate) fn spill_for_group<'r>(&self, kidx: &[usize], n_rows: usize, rows: impl IntoIterator<Item = &'r [TermId]>) -> Option<spill::Partitions> {
        if kidx.is_empty() { return None }
        let (dir, parts) = self.spill_plan(n_rows)?;
        let spilled = spill::Partitions::write(&dir, self.vars.len(), kidx, parts, rows, || self.check()).unwrap_or_else(|e| storage_failure(e));
        phase::spilled(spilled.bytes());
        Some(spilled)
    }

    /// Groups each partition with `group`, which must emit one row per group with its key at `kidx`, in first-seen
    /// order, and merges the partitions' groups back into first-seen order.
    pub(crate) fn group_partitions(&self, spilled: &spill::Partitions, kidx: &[usize], group: impl Fn(Vec<Row>) -> Vec<Row>) -> Vec<Row> {
        let key = |r: &[TermId]| -> Vec<TermId> { kidx.iter().map(|&i| r[i]).collect() };
        let mut out: Vec<(u64, Row)> = Vec::new();
        for p in 0..spilled.count() {
            self.check();
            let (seqs, rows) = spilled.read(p).unwrap_or_else(|e| storage_failure(e));
            if rows.is_empty() { continue }
            let mut first: HashMap<Vec<TermId>, u64> = HashMap::new();
            for (s, r) in seqs.iter().zip(&rows) { first.entry(key(r)).or_insert(*s); }
            drop(seqs);
            for g in group(rows) { out.push((first[&key(&g)], g)) }
        }
        out.sort_unstable_by_key(|(s, _)| *s);
        out.into_iter().map(|(_, r)| r).collect()
    }
}
