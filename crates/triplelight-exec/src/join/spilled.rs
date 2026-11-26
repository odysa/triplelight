//! `join_rows` under the memory budget (#111). When the build side and its table may not fit, both sides are written
//! to run files partitioned by the same hash of the join key (`spill::partition` with one hasher), the in-memory rows are
//! dropped, and each partition pair is joined on its own: compatible mappings agree on the key, so they meet in the
//! same partition. The answer comes out in `join_rows`'s order (probe rows in input order, each one's matches in
//! build-row order): every output row is tagged with its (probe, build) input positions and the partitions' rows are
//! merged on them.
use super::{join_key, join_rows, merge_compatible};
use crate::limits::spill;
use crate::model::TermId;
use crate::storage::Snapshot;
use crate::{storage_failure, Ctx, Row};
use std::collections::HashMap;

impl<S: Snapshot + ?Sized> Ctx<'_, S> {
    /// `join_rows(left, right)` charging the build table, or spilling both sides by key partition when the build side
    /// and its table do not fit what is free (and the budget can spill). Answers and their order are `join_rows`'s.
    pub(crate) fn join_within_budget(&self, left: Vec<Row>, right: Vec<Row>) -> Vec<Row> {
        let entry = self.row_bytes() + 32;
        if !left.is_empty() && !right.is_empty() && left.len().saturating_mul(right.len()) > 64
            && self.budget.is_some_and(|b| b.spill_dir_for(right.len(), (right.len() as u64 * entry).saturating_mul(2)).is_some()) {
            let key = join_key(&left, &right);
            if !key.is_empty() { return self.join_spilled(&key, left, right) }
        }
        let _table = self.reserve(right.len() as u64 * entry);
        join_rows(left, right)
    }

    fn join_spilled(&self, key: &[usize], left: Vec<Row>, right: Vec<Row>) -> Vec<Row> {
        let budget = self.budget.expect("spill needs a budget");
        let dir = budget.scratch_dir().expect("spill needs a scratch directory");
        let bytes = (left.len() + right.len()) as u64 * self.row_bytes();
        let (parts, buffer) = spill::partitions_for(bytes, budget.available());
        let hasher = std::hash::RandomState::new();
        let (l, r) = {
            // Both sides' writers at once: twice the buffers.
            let _buffers = self.reserve(2 * (parts * buffer) as u64);
            let write = |rows: &[Row]| {
                let records = rows.iter().enumerate().map(|(seq, row)| (spill::partition(&hasher, key, row, parts), seq as u64, row.as_slice()));
                spill::Spill::write(dir, self.vars.len(), (parts, buffer), records, || self.check()).unwrap_or_else(|e| storage_failure(e))
            };
            (write(&left), write(&right))
        };
        drop((left, right));
        let mut held = self.reserve(0);
        let mut out: Vec<((u64, u64), Row)> = Vec::new();
        let mut k: Vec<TermId> = Vec::with_capacity(key.len());
        for p in 0..parts {
            self.check();
            if l.rows(p) == 0 || r.rows(p) == 0 { continue }
            // The partition pair and the build table are held while they are joined.
            let _part = self.reserve(l.rows(p) * (self.row_bytes() + 8) + r.rows(p) * (self.row_bytes() + 8 + 32));
            let (rseqs, rrows): (Vec<u64>, Vec<Row>) = r.read(p, 1 << 16).unwrap_or_else(|e| storage_failure(e)).map(|x| x.unwrap_or_else(|e| storage_failure(e))).unzip();
            let mut table: HashMap<Vec<TermId>, Vec<usize>> = HashMap::new();
            for (j, b) in rrows.iter().enumerate() { table.entry(key.iter().map(|&i| b[i]).collect()).or_default().push(j) }
            let (lseqs, lrows): (Vec<u64>, Vec<Row>) = l.read(p, 1 << 16).unwrap_or_else(|e| storage_failure(e)).map(|x| x.unwrap_or_else(|e| storage_failure(e))).unzip();
            for (n, (ls, a)) in lseqs.iter().zip(&lrows).enumerate() {
                if n % 4096 == 0 { self.check() }
                k.clear();
                k.extend(key.iter().map(|&i| a[i]));
                let Some(matches) = table.get(k.as_slice()) else { continue };
                for &j in matches {
                    if let Some(m) = merge_compatible(a, &rrows[j]) {
                        out.push(((*ls, rseqs[j]), m));
                        if out.len().is_multiple_of(4096) { Self::resize(&mut held, out.len() as u64 * (self.row_bytes() + 16)) }
                    }
                }
            }
        }
        Self::resize(&mut held, out.len() as u64 * (self.row_bytes() + 16));
        out.sort_unstable_by_key(|(s, _)| *s);
        out.into_iter().map(|(_, r)| r).collect()
    }
}
