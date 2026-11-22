//! Serving SOP and OPS from the orders a disk base keeps (task #73).
//!
//! A base may keep only SPO, PSO, POS and OSP. Every request on SOP or OPS is still answered with the same rows in
//! the same order, so the `Snapshot` contract (and the planner and executor) do not change:
//!
//! - With the first two key columns bound, SOP `[s,o,..]` is OSP `[o,s,..]` and OPS `[o,p,..]` is POS `[p,o,..]`:
//!   the third column is the same and already sorted, so scans, counts and seeks cost what they do on a kept order.
//! - Otherwise the request is read from the kept order with the same first column (SPO for SOP, OSP for OPS),
//!   one first-column group at a time, and each group is sorted by the wanted key. Groups ascend by the shared first
//!   column, so the concatenation is sorted. Memory is one group; time is the rows read plus the group sorts. With
//!   no prefix this reads the whole table.
use crate::model::{Perm, TermId, TripleBatch};
use crate::storage::StorageError;

/// The orders a base serves directly. A base implements this; the free functions answer every order from it.
pub(super) trait KeptOrders {
    fn has(&self, perm: Perm) -> bool;
    fn scan_kept<'a>(&'a self, perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a>;
    fn count_kept(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError>;
    fn seek_kept(&self, perm: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError>;
}

/// How a missing order is served: `twin` holds the same rows with its first two key columns swapped; `sibling`
/// shares the first key column, and the other two are swapped.
fn route(perm: Perm) -> (Perm, Perm) {
    match perm {
        Perm::Sop => (Perm::Osp, Perm::Spo),
        Perm::Ops => (Perm::Pos, Perm::Osp),
        _ => unreachable!("only SOP and OPS are derived; a base keeps {perm:?}"),
    }
}

fn swapped(prefix: &[TermId], a: usize, b: usize) -> Vec<TermId> {
    let mut key = prefix.to_vec();
    if b < key.len() { key.swap(a, b) }
    key
}

pub(super) fn scan<'a, O: KeptOrders>(o: &'a O, perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
    if o.has(perm) { return o.scan_kept(perm, prefix) }
    let (twin, sibling) = route(perm);
    if prefix.len() >= 2 {
        return Box::new(o.scan_kept(twin, &swapped(prefix, 0, 1)).map(|b| b.map(|mut b| { b.cols.swap(0, 1); b })));
    }
    Box::new(Regroup { rows: o.scan_kept(sibling, prefix), pending: Vec::new(), group: Vec::new(), done: false })
}

pub(super) fn count<O: KeptOrders>(o: &O, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> {
    if o.has(perm) { return o.count_kept(perm, prefix) }
    let (twin, sibling) = route(perm);
    if prefix.len() >= 2 { o.count_kept(twin, &swapped(prefix, 0, 1)) } else { o.count_kept(sibling, prefix) }
}

pub(super) fn seek<O: KeptOrders>(o: &O, perm: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> {
    if o.has(perm) { return o.seek_kept(perm, prefix, target) }
    let (twin, sibling) = route(perm);
    match prefix.len() {
        // The first key column is the sibling's first column.
        0 => o.seek_kept(sibling, prefix, target),
        // The second key column is the sibling's third: the smallest value >= target in the group.
        1 => {
            let mut best: Option<TermId> = None;
            for batch in o.scan_kept(sibling, prefix) {
                for &v in &batch?.cols[2] {
                    if v >= target && best.is_none_or(|b| v < b) { best = Some(v) }
                }
            }
            Ok(best)
        }
        _ => o.seek_kept(twin, &swapped(prefix, 0, 1), target),
    }
}

/// Rows of the sibling order (first column shared), regrouped: within each first-column group, sorted by the wanted
/// key (the sibling's columns 0, 2, 1), with columns 1 and 2 swapped back into the wanted key order.
struct Regroup<'a> {
    rows: Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a>,
    /// Sorted rows ready to be returned.
    pending: Vec<[TermId; 3]>,
    /// The current, still growing first-column group, in wanted key order.
    group: Vec<[TermId; 3]>,
    done: bool,
}
impl Regroup<'_> {
    fn close_group(&mut self) {
        self.group.sort_unstable();
        self.pending.append(&mut self.group);
    }
    fn batch(&mut self) -> TripleBatch {
        let n = self.pending.len().min(4096);
        let mut batch = TripleBatch::default();
        for row in self.pending.drain(..n) {
            for (col, v) in batch.cols.iter_mut().zip(row) { col.push(v) }
        }
        batch
    }
}
impl Iterator for Regroup<'_> {
    type Item = Result<TripleBatch, StorageError>;
    fn next(&mut self) -> Option<Self::Item> {
        while self.pending.len() < 4096 && !self.done {
            match self.rows.next() {
                None => { self.done = true; self.close_group() }
                Some(Err(e)) => { self.done = true; self.pending.clear(); self.group.clear(); return Some(Err(e)) }
                Some(Ok(b)) => for i in 0..b.len() {
                    let row = [b.cols[0][i], b.cols[2][i], b.cols[1][i]];
                    if self.group.first().is_some_and(|g| g[0] != row[0]) { self.close_group() }
                    self.group.push(row);
                },
            }
        }
        (!self.pending.is_empty()).then(|| Ok(self.batch()))
    }
}
