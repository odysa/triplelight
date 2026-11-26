//! GROUP BY over an input that may not fit in the memory budget (#111): the input rows are hash-partitioned on the
//! group keys into run files under the budget's scratch directory (`spill::Partitions`), the in-memory input is
//! dropped, and each partition is grouped on its own, so one partition's rows and groups are in memory at a time. A
//! key lives in exactly one partition, so every group is complete. Groups come out in first-seen order, as from the
//! in-memory path: each is tagged with the input position of its first row and the partitions' groups are merged on it.
//!
//! When to spill: the fast paths (`count_one_key`, the fast aggregates) only borrow their input, so they run in memory
//! first and spill only if that ends with `ResourceLimit` (a shape that fits never spills). The general path moves its
//! rows into the groups, so it decides up front: it spills when twice the input does not fit what is free. A partition
//! that still does not fit (most rows under one key) ends the query with `ResourceLimit`, as without spilling.
use super::super::{storage_failure, Ctx, Row};
use crate::limits::{spill, OverBudgetStop};
use crate::model::TermId;
use crate::storage::Snapshot;
use std::collections::HashSet;

impl<S: Snapshot + ?Sized> Ctx<'_, S> {
    /// For the general path, which holds its input once more in the groups: spill when twice the input does not fit.
    pub(crate) fn should_spill_general(&self, rows: usize) -> bool {
        self.budget.is_some_and(|b| b.spill_dir_for(rows, (rows as u64 * self.row_bytes()).saturating_mul(2)).is_some())
    }

    /// Whether the input of `rows` rows must spill even if it fits (`MemoryBudget::with_spill_rows`).
    pub(crate) fn forced_spill(&self, rows: usize) -> bool { self.budget.is_some_and(|b| b.forces_spill(rows)) }

    /// Runs `f` in memory. `None` if it ran out of budget and the query can spill instead (`spillable` and a scratch
    /// directory); its reservations were returned by the unwind. Other stops (cancel, timeout, storage) pass through.
    pub(crate) fn in_memory_or_spill<T>(&self, spillable: bool, f: impl FnOnce() -> T) -> Option<T> {
        if !(spillable && self.budget.is_some_and(|b| b.scratch_dir().is_some())) { return Some(f()) }
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            Ok(v) => Some(v),
            Err(p) if p.is::<OverBudgetStop>() => None,
            Err(p) => std::panic::resume_unwind(p),
        }
    }

    /// Writes a GROUP BY input of `n_rows` rows (each `vars.len()` IDs, keys at `kidx`) to disk by key partition. The
    /// caller then drops its in-memory rows and calls `group_partitions`. The write buffers are charged while it runs.
    pub(crate) fn spill_for_group<'r>(&self, kidx: &[usize], n_rows: usize, rows: impl IntoIterator<Item = &'r [TermId]>) -> spill::Spill {
        let budget = self.budget.expect("spill needs a budget");
        let dir = budget.scratch_dir().expect("spill needs a scratch directory");
        let (parts, buffer) = spill::partitions_for(n_rows as u64 * self.row_bytes(), budget.available());
        let _buffers = self.reserve((parts * buffer) as u64);
        let hasher = std::hash::RandomState::new();
        let records = rows.into_iter().enumerate().map(|(seq, row)| (spill::partition(&hasher, kidx, row, parts), seq as u64, row));
        spill::Spill::write(dir, self.vars.len(), (parts, buffer), records, || self.check()).unwrap_or_else(|e| storage_failure(e))
    }

    /// Groups each partition with `group`, which must emit one row per group, in first-seen order, and merges the
    /// partitions' groups back into first-seen order. The merged result is charged as it grows.
    pub(crate) fn group_partitions(&self, spilled: &spill::Spill, kidx: &[usize], group: impl Fn(Vec<Row>) -> Vec<Row>) -> Vec<Row> {
        let mut held = self.reserve(0);
        let mut out: Vec<(u64, Row)> = Vec::new();
        let mut key: Vec<TermId> = Vec::with_capacity(kidx.len());
        for p in 0..spilled.files() {
            self.check();
            // One partition's rows (and their input positions) are about to be in memory: held while they are read and
            // their groups' first positions found, which ends the query up front if they cannot be. `group` then
            // charges what it builds itself, as on the in-memory path, where the input is checked but not held.
            let n = spilled.rows(p);
            let mut part = self.reserve(n * (self.row_bytes() + 8));
            let (seqs, rows): (Vec<u64>, Vec<Row>) = spilled.read(p, 1 << 16).unwrap_or_else(|e| storage_failure(e))
                .map(|r| r.unwrap_or_else(|e| storage_failure(e))).unzip();
            if rows.is_empty() { continue }
            // Each group's first input position, in first-seen order (the order `group` emits them in).
            let mut seen: HashSet<Vec<TermId>> = HashSet::new();
            let mut firsts: Vec<u64> = Vec::new();
            let key_bytes = 8 * kidx.len() as u64 + 48;
            for (s, r) in seqs.iter().zip(&rows) {
                key.clear();
                key.extend(kidx.iter().map(|&i| r[i]));
                if !seen.contains(key.as_slice()) {
                    seen.insert(key.clone());
                    firsts.push(*s);
                    if firsts.len().is_multiple_of(4096) { Self::resize(&mut part, n * (self.row_bytes() + 8) + firsts.len() as u64 * (key_bytes + 8)) }
                }
            }
            drop((seen, seqs));
            Self::resize(&mut part, firsts.len() as u64 * 8);
            let groups = group(rows);
            assert_eq!(groups.len(), firsts.len(), "a grouping emits one row per group");
            out.extend(firsts.into_iter().zip(groups));
            Self::resize(&mut held, out.len() as u64 * (self.row_bytes() + 8));
        }
        out.sort_unstable_by_key(|(s, _)| *s);
        out.into_iter().map(|(_, r)| r).collect()
    }
}
