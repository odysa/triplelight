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

/// Coverage counters for the derived-order paths (pilot 2, task #98). Off unless `TRIPLELIGHT_TRACE_DERIVED` is set;
/// when off, each call costs one relaxed load. Logging only: no answer or plan depends on them.
pub mod trace {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    use std::sync::OnceLock;
    pub(crate) fn on() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("TRIPLELIGHT_TRACE_DERIVED").is_some())
    }
    macro_rules! counters { ($($name:ident),*) => {
        $(pub(crate) static $name: AtomicU64 = AtomicU64::new(0);)*
        /// Counter values since the last `take`, then reset (one line per query in the server).
        pub fn take() -> String {
            let mut s = String::new();
            $( s.push_str(&format!("{}={} ", stringify!($name).to_lowercase(), $name.swap(0, Relaxed))); )*
            s.pop(); s
        }
    } }
    counters!(REMAP_CALLS, REGROUP_SCANS, REGROUP_ROWS, REGROUP_GROUPS, MAX_GROUP, MEMO_HITS, MEMO_MISSES, MEMO_ROWS, SORT_NS);
    pub(crate) fn add(c: &AtomicU64, n: u64) { if on() { c.fetch_add(n, Relaxed); } }
    pub(crate) fn max(c: &AtomicU64, n: u64) { if on() { c.fetch_max(n, Relaxed); } }
    pub fn enabled() -> bool { on() }
}

/// The orders a base serves directly. A base implements this; the free functions answer every order from it.
pub(super) trait KeptOrders {
    fn has(&self, perm: Perm) -> bool;
    fn scan_kept<'a>(&'a self, perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a>;
    fn count_kept(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError>;
    fn seek_kept(&self, perm: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError>;
    /// The memo for one-column-prefix seeks on a derived order.
    fn seek_memo(&self) -> &SeekMemo;
}

/// The sorted distinct second-column values of the last group a one-column-prefix seek on a derived order read.
/// A leapfrog join seeks one group many times with rising targets, so without it each seek would rescan the group
/// (quadratic in the group size); with it, a group is read once and each seek is a binary search.
#[derive(Default)]
pub(super) struct SeekMemo(std::sync::Mutex<Option<(Perm, TermId, std::sync::Arc<Vec<TermId>>)>>);

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
        trace::add(&trace::REMAP_CALLS, 1);
        return Box::new(o.scan_kept(twin, &swapped(prefix, 0, 1)).map(|b| b.map(|mut b| { b.cols.swap(0, 1); b })));
    }
    trace::add(&trace::REGROUP_SCANS, 1);
    Box::new(Regroup { rows: o.scan_kept(sibling, prefix), pending: Vec::new(), group: Vec::new(), done: false })
}

pub(super) fn count<O: KeptOrders>(o: &O, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> {
    if o.has(perm) { return o.count_kept(perm, prefix) }
    let (twin, sibling) = route(perm);
    if prefix.len() >= 2 { trace::add(&trace::REMAP_CALLS, 1); o.count_kept(twin, &swapped(prefix, 0, 1)) } else { o.count_kept(sibling, prefix) }
}

pub(super) fn seek<O: KeptOrders>(o: &O, perm: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> {
    if o.has(perm) { return o.seek_kept(perm, prefix, target) }
    let (twin, sibling) = route(perm);
    match prefix.len() {
        // The first key column is the sibling's first column.
        0 => o.seek_kept(sibling, prefix, target),
        // The second key column is the sibling's third: the smallest value >= target in the group (memoized).
        1 => {
            let memo = &o.seek_memo().0;
            let cached = memo.lock().unwrap().as_ref().filter(|(p, g, _)| *p == perm && *g == prefix[0]).map(|(.., v)| std::sync::Arc::clone(v));
            let values = match cached {
                Some(v) => { trace::add(&trace::MEMO_HITS, 1); v }
                None => {
                    let mut v = Vec::new();
                    for batch in o.scan_kept(sibling, prefix) { v.extend_from_slice(&batch?.cols[2]) }
                    trace::add(&trace::MEMO_MISSES, 1);
                    trace::add(&trace::MEMO_ROWS, v.len() as u64);
                    trace::max(&trace::MAX_GROUP, v.len() as u64);
                    v.sort_unstable();
                    v.dedup();
                    let v = std::sync::Arc::new(v);
                    *memo.lock().unwrap() = Some((perm, prefix[0], std::sync::Arc::clone(&v)));
                    v
                }
            };
            Ok(values.get(values.partition_point(|&v| v < target)).copied())
        }
        _ => { trace::add(&trace::REMAP_CALLS, 1); o.seek_kept(twin, &swapped(prefix, 0, 1), target) }
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
        let t = trace::on().then(std::time::Instant::now);
        trace::add(&trace::REGROUP_GROUPS, 1);
        trace::add(&trace::REGROUP_ROWS, self.group.len() as u64);
        trace::max(&trace::MAX_GROUP, self.group.len() as u64);
        self.group.sort_unstable();
        if let Some(t) = t { trace::add(&trace::SORT_NS, t.elapsed().as_nanos() as u64) }
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
