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
    counters!(REMAP_CALLS, REGROUP_SCANS, REGROUP_ROWS, REGROUP_GROUPS, MAX_GROUP, MEMO_HITS, MEMO_MISSES, MEMO_ROWS, SORT_NS,
              REGROUP_SPILLS, SPILL_ROWS, PEAK_SPILL_BYTES, GROUP_HITS, GROUP_MISSES);
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
    /// Where a large regroup spills (the store's directory).
    fn scratch_dir(&self) -> &std::path::Path;
}

/// The sorted distinct second-column values of the last group a one-column-prefix seek on a derived order read.
/// A leapfrog join seeks one group many times with rising targets, so without it each seek would rescan the group
/// (quadratic in the group size); with it, a group is read once and each seek is a binary search.
///
/// It also keeps the last few *regrouped* groups of one-column-prefix scans on a derived order (most recent last).
/// Repeated probes on one key, such as many requests on one class object through OPS, then sort the group once
/// instead of once per probe. Bounded in total rows by `spill_rows()` (the in-memory sort limit); a larger group is
/// never cached and regroups (and spills) as before.
#[derive(Default)]
pub(super) struct SeekMemo(
    std::sync::Mutex<Option<(Perm, TermId, std::sync::Arc<Vec<TermId>>)>>,
    std::sync::Mutex<std::collections::VecDeque<(Perm, TermId, std::sync::Arc<Vec<[TermId; 3]>>)>>,
);
/// The most groups the regrouped-group cache holds, whatever their size.
const GROUP_CACHE_ENTRIES: usize = 8;
impl SeekMemo {
    fn group(&self, perm: Perm, key: TermId) -> Option<std::sync::Arc<Vec<[TermId; 3]>>> {
        let mut groups = self.1.lock().unwrap();
        let i = groups.iter().position(|(p, k, _)| *p == perm && *k == key)?;
        let entry = groups.remove(i).expect("position is in range");
        let rows = std::sync::Arc::clone(&entry.2);
        groups.push_back(entry);
        Some(rows)
    }
    fn insert_group(&self, perm: Perm, key: TermId, rows: std::sync::Arc<Vec<[TermId; 3]>>, limit: usize) {
        let mut groups = self.1.lock().unwrap();
        if groups.iter().any(|(p, k, _)| *p == perm && *k == key) { return }
        let mut total = rows.len() + groups.iter().map(|g| g.2.len()).sum::<usize>();
        while !groups.is_empty() && (total > limit || groups.len() >= GROUP_CACHE_ENTRIES) {
            total -= groups.pop_front().expect("not empty").2.len();
        }
        groups.push_back((perm, key, rows));
    }
}

/// Batches of up to 4096 rows from a shared, already sorted group.
fn group_batches(rows: std::sync::Arc<Vec<[TermId; 3]>>) -> impl Iterator<Item = Result<TripleBatch, StorageError>> {
    let mut pos = 0;
    std::iter::from_fn(move || {
        if pos >= rows.len() { return None }
        let end = (pos + 4096).min(rows.len());
        let mut batch = TripleBatch::default();
        for c in &mut batch.cols { c.reserve(end - pos) }
        for row in &rows[pos..end] { for (col, v) in batch.cols.iter_mut().zip(row) { col.push(*v) } }
        pos = end;
        Some(Ok(batch))
    })
}

/// One group of `perm` (first column `key`) regrouped from `sibling` and sorted in memory; `None` if the group is
/// larger than `limit` rows (it then goes through `Regroup`, which may spill).
fn regroup_in_memory<O: KeptOrders>(o: &O, perm: Perm, sibling: Perm, key: TermId, limit: usize) -> Result<Option<std::sync::Arc<Vec<[TermId; 3]>>>, StorageError> {
    let n = o.count_kept(sibling, &[key])?;
    if n > limit as u64 { return Ok(None) }
    let t = trace::on().then(std::time::Instant::now);
    let mut rows = Vec::with_capacity(n as usize);
    for batch in o.scan_kept(sibling, &[key]) {
        let b = batch?;
        rows.extend((0..b.len()).map(|i| [b.cols[0][i], b.cols[2][i], b.cols[1][i]]));
    }
    rows.sort_unstable();
    trace::add(&trace::REGROUP_GROUPS, 1);
    trace::add(&trace::REGROUP_ROWS, rows.len() as u64);
    trace::max(&trace::MAX_GROUP, rows.len() as u64);
    if let Some(t) = t { trace::add(&trace::SORT_NS, t.elapsed().as_nanos() as u64) }
    let rows = std::sync::Arc::new(rows);
    o.seek_memo().insert_group(perm, key, std::sync::Arc::clone(&rows), limit);
    Ok(Some(rows))
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
        trace::add(&trace::REMAP_CALLS, 1);
        return Box::new(o.scan_kept(twin, &swapped(prefix, 0, 1)).map(|b| b.map(|mut b| { b.cols.swap(0, 1); b })));
    }
    trace::add(&trace::REGROUP_SCANS, 1);
    if let [key] = *prefix {
        if let Some(rows) = o.seek_memo().group(perm, key) {
            trace::add(&trace::GROUP_HITS, 1);
            return Box::new(group_batches(rows));
        }
        trace::add(&trace::GROUP_MISSES, 1);
        match regroup_in_memory(o, perm, sibling, key, spill_rows()) {
            Ok(Some(rows)) => return Box::new(group_batches(rows)),
            Ok(None) => {}
            Err(e) => return Box::new(std::iter::once(Err(e))),
        }
    }
    Box::new(Regroup::new(o.scan_kept(sibling, prefix), spill_dir(o.scratch_dir())))
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
                None => if let Some(rows) = o.seek_memo().group(perm, prefix[0]) {
                    // The regrouped group is cached: its second column is already sorted.
                    trace::add(&trace::GROUP_HITS, 1);
                    let mut v: Vec<TermId> = rows.iter().map(|r| r[1]).collect();
                    v.dedup();
                    let v = std::sync::Arc::new(v);
                    *memo.lock().unwrap() = Some((perm, prefix[0], std::sync::Arc::clone(&v)));
                    v
                } else {
                    // Distinct values only, collected as they come: memory is the number of distinct second-column
                    // values (an object's predicates for OPS, a subject's objects for SOP), not the group's rows.
                    let (mut set, mut rows) = (std::collections::BTreeSet::new(), 0u64);
                    for batch in o.scan_kept(sibling, prefix) {
                        let batch = batch?;
                        rows += batch.len() as u64;
                        set.extend(batch.cols[2].iter().copied());
                    }
                    trace::add(&trace::MEMO_MISSES, 1);
                    trace::add(&trace::MEMO_ROWS, rows);
                    trace::max(&trace::MAX_GROUP, rows);
                    let v: Vec<TermId> = set.into_iter().collect();
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
///
/// Memory is bounded: a group is sorted in memory up to `spill_rows()` rows; a larger one (one very common object,
/// such as a class on `rdf:type`) is pushed through the build's fixed-width external sorter (#90) in a private scratch
/// directory under the system temporary directory (`TMPDIR`), removed when the group is done or the iterator is dropped,
/// and streamed back in order. The output is the same either way.
struct Regroup<'a> {
    rows: Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a>,
    /// Finished groups in output order: runs of in-memory groups, and spilled groups between them.
    out: std::collections::VecDeque<Segment>,
    /// Rows in `out` (in-memory segments only), to know when 4096 are ready without counting.
    ready: usize,
    /// The first column of the current group, and its rows while it fits in memory.
    key: Option<TermId>,
    group: Vec<[TermId; 3]>,
    /// The current group once it outgrew `spill_limit` rows: its rows go to the external sorter instead of `group`.
    spill: Option<(super::fixed_sort::FixedSort<24>, u64)>,
    spill_limit: usize,
    spill_dir: std::path::PathBuf,
    done: bool,
}
/// Finished, sorted rows: in memory (returned from `pos` on), or a spilled group streamed from the sorter's output (the
/// sorter owns the scratch, removed when the segment is dropped).
enum Segment {
    Mem(Vec<[TermId; 3]>, usize),
    Spilled(#[allow(dead_code)] super::fixed_sort::FixedSort<24>, std::io::BufReader<std::fs::File>), // the sorter is kept for its scratch
}
/// The external sorter's own budget for one spilled group.
const SPILL_SORT_BYTES: usize = 64 * 1024 * 1024;
/// The smallest group that may spill: below this, sorting in memory is cheap whatever the budget.
const SPILL_FLOOR_ROWS: usize = 1 << 20;
#[cfg(test)]
thread_local! {
    static SPILL_ROWS_ON_THIS_THREAD: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static SPILL_DIR_ON_THIS_THREAD: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
}
/// Tests: spill every group of more than `rows` rows that regroups on this thread, into `dir` (default: TMPDIR).
#[cfg(test)]
pub(super) fn spill_on_this_thread(rows: Option<usize>, dir: Option<std::path::PathBuf>) {
    SPILL_ROWS_ON_THIS_THREAD.with(|c| c.set(rows));
    SPILL_DIR_ON_THIS_THREAD.with(|c| *c.borrow_mut() = dir);
}
/// Where a group spills, given the base directory: the store's `scratch/` directory beside its bases (on the store's
/// device and inside the tree S3 samples; bases themselves stay immutable), created on demand and emptied by
/// `Store::open`. A base opened on its own (no store around it), or a store whose root is read-only, falls back to
/// `TRIPLELIGHT_SCRATCH_DIR`, then to the system temporary directory (often a tmpfs, whose pages a cgroup charges as
/// memory, so it is the last resort), logged once. The query never fails for lack of a place to spill.
fn spill_dir(base: &std::path::Path) -> std::path::PathBuf {
    #[cfg(test)]
    if let Some(d) = SPILL_DIR_ON_THIS_THREAD.with(|c| c.borrow().clone()) { return d }
    if let Some(root) = base.parent().filter(|r| r.join("CURRENT").is_file()) {
        let scratch = root.join(crate::store::SCRATCH_DIR);
        if std::fs::create_dir_all(&scratch).is_ok() { return scratch }
    }
    static LOGGED: std::sync::Once = std::sync::Once::new();
    let fallback = std::env::var_os("TRIPLELIGHT_SCRATCH_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    LOGGED.call_once(|| eprintln!("derived orders: no store scratch directory for {}; large regroups spill to {}", base.display(), fallback.display()));
    fallback
}
/// The largest group sorted in memory: a quarter of the block-cache budget in rows, so one group never needs more than
/// a fraction of what the store may use, and never less than `SPILL_FLOOR_ROWS`.
fn spill_rows() -> usize {
    #[cfg(test)]
    if let Some(rows) = SPILL_ROWS_ON_THIS_THREAD.with(|c| c.get()) { return rows }
    // Measurement override: `TRIPLELIGHT_REGROUP_SPILL_ROWS=n` (e.g. a huge n to compare with no spill at the same budget).
    if let Some(n) = std::env::var("TRIPLELIGHT_REGROUP_SPILL_ROWS").ok().and_then(|v| v.parse().ok()) { return n }
    let budget = super::disk_index::cache_budget_mb().saturating_mul(1 << 20);
    (budget / 4 / 24).max(SPILL_FLOOR_ROWS)
}
fn encode(row: [TermId; 3]) -> [u8; 24] {
    let mut r = [0u8; 24];
    for (k, id) in row.iter().enumerate() { r[k * 8..k * 8 + 8].copy_from_slice(&id.0.to_be_bytes()) }
    r
}
fn decode(r: &[u8; 24]) -> [TermId; 3] {
    let id = |k: usize| TermId(u64::from_be_bytes(r[k * 8..k * 8 + 8].try_into().unwrap()));
    [id(0), id(1), id(2)]
}
impl<'a> Regroup<'a> {
    fn new(rows: Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a>, spill_dir: std::path::PathBuf) -> Self {
        Self { rows, out: Default::default(), ready: 0, key: None, group: Vec::new(), spill: None, spill_limit: spill_rows(), spill_dir, done: false }
    }
    fn add(&mut self, row: [TermId; 3]) -> Result<(), StorageError> {
        if let Some((sort, n)) = &mut self.spill { *n += 1; return sort.push(encode(row)) }
        self.group.push(row);
        if self.group.len() > self.spill_limit {
            // Outgrew memory: move what is buffered to the external sorter and keep pushing there.
            let mut sort = super::fixed_sort::FixedSort::<24>::new(&self.spill_dir, SPILL_SORT_BYTES)?;
            let n = self.group.len() as u64;
            for row in self.group.drain(..) { sort.push(encode(row))? }
            self.group = Vec::new();
            self.spill = Some((sort, n));
        }
        Ok(())
    }
    /// Ends the current group: an in-memory group is sorted onto the last in-memory segment; a spilled one is finished
    /// and queued as its own segment, so groups come out in order.
    fn close_group(&mut self) -> Result<(), StorageError> {
        if self.key.take().is_none() { return Ok(()) }
        let t = trace::on().then(std::time::Instant::now);
        trace::add(&trace::REGROUP_GROUPS, 1);
        if let Some((mut sort, n)) = self.spill.take() {
            trace::add(&trace::REGROUP_ROWS, n);
            trace::max(&trace::MAX_GROUP, n);
            trace::add(&trace::REGROUP_SPILLS, 1);
            trace::add(&trace::SPILL_ROWS, n);
            trace::max(&trace::PEAK_SPILL_BYTES, n * 24);
            let path = sort.finish()?;
            let reader = std::io::BufReader::with_capacity(1 << 16, std::fs::File::open(path)?);
            self.out.push_back(Segment::Spilled(sort, reader));
        } else {
            trace::add(&trace::REGROUP_ROWS, self.group.len() as u64);
            trace::max(&trace::MAX_GROUP, self.group.len() as u64);
            self.group.sort_unstable();
            self.ready += self.group.len();
            match self.out.back_mut() {
                Some(Segment::Mem(rows, _)) => rows.append(&mut self.group),
                _ => self.out.push_back(Segment::Mem(std::mem::take(&mut self.group), 0)),
            }
        }
        if let Some(t) = t { trace::add(&trace::SORT_NS, t.elapsed().as_nanos() as u64) }
        Ok(())
    }
    /// Up to 4096 rows from the front segment; `None` when nothing is ready.
    fn batch(&mut self) -> Result<Option<TripleBatch>, StorageError> {
        let mut batch = TripleBatch::default();
        while let Some(seg) = self.out.front_mut() {
            match seg {
                Segment::Mem(rows, pos) => {
                    let n = (rows.len() - *pos).min(4096);
                    for row in &rows[*pos..*pos + n] {
                        for (col, v) in batch.cols.iter_mut().zip(row) { col.push(*v) }
                    }
                    *pos += n;
                    self.ready -= n;
                    if *pos == rows.len() { self.out.pop_front(); }
                    // Later groups keep being appended to this segment while it is read, so drop the consumed front
                    // once it is the larger half (amortized O(1) per row; the segment never holds the whole scan).
                    else if *pos > rows.len() / 2 { rows.drain(..*pos); *pos = 0 }
                }
                Segment::Spilled(_, reader) => {
                    let mut r = [0u8; 24];
                    while batch.len() < 4096 && super::fixed_sort::read_fixed(reader, &mut r)? {
                        for (col, v) in batch.cols.iter_mut().zip(decode(&r)) { col.push(v) }
                    }
                    if batch.is_empty() { self.out.pop_front(); continue } // drops the sorter and its scratch
                }
            }
            if !batch.is_empty() { return Ok(Some(batch)) }
        }
        Ok(None)
    }
}
impl Iterator for Regroup<'_> {
    type Item = Result<TripleBatch, StorageError>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
        // Read until 4096 rows are ready, a spilled group is queued, or the input ends.
        while self.ready < 4096 && !self.done && !self.out.iter().any(|s| matches!(s, Segment::Spilled(..))) {
            let step = match self.rows.next() {
                None => { self.done = true; self.close_group() }
                Some(Err(e)) => Err(e),
                Some(Ok(b)) => (|| {
                    for i in 0..b.len() {
                        let row = [b.cols[0][i], b.cols[2][i], b.cols[1][i]];
                        if self.key.is_some_and(|k| k != row[0]) { self.close_group()? }
                        self.key = Some(row[0]);
                        self.add(row)?;
                    }
                    Ok(())
                })(),
            };
            if let Err(e) = step { self.fail(); return Some(Err(e)) }
        }
        match self.batch() {
            Ok(Some(b)) => return Some(Ok(b)),
            // A spilled group was emptied with input left to read: read on.
            Ok(None) if !self.done => continue,
            Ok(None) => return None,
            Err(e) => { self.fail(); return Some(Err(e)) }
        }
        }
    }
}
impl Regroup<'_> {
    /// After an error: nothing more is returned (and dropping the sorters removes their scratch).
    fn fail(&mut self) {
        self.done = true;
        self.out.clear();
        self.ready = 0;
        self.group.clear();
        self.spill = None;
    }
}

#[cfg(test)]
mod group_cache_tests {
    use super::*;
    fn rows(n: usize) -> std::sync::Arc<Vec<[TermId; 3]>> { std::sync::Arc::new(vec![[TermId(1), TermId(2), TermId(3)]; n]) }
    #[test]
    fn the_group_cache_is_bounded_in_rows_and_entries_and_least_recent_goes_first() {
        let memo = SeekMemo::default();
        memo.insert_group(Perm::Ops, TermId(1), rows(40), 100);
        memo.insert_group(Perm::Ops, TermId(2), rows(40), 100);
        assert!(memo.group(Perm::Ops, TermId(1)).is_some(), "a hit makes key 1 the most recent");
        memo.insert_group(Perm::Ops, TermId(3), rows(40), 100); // 120 > 100: evicts key 2, the least recent
        assert!(memo.group(Perm::Ops, TermId(2)).is_none());
        assert!(memo.group(Perm::Ops, TermId(1)).is_some() && memo.group(Perm::Ops, TermId(3)).is_some());
        assert!(memo.group(Perm::Sop, TermId(1)).is_none(), "keyed by order as well as by value");
        for k in 10..10 + GROUP_CACHE_ENTRIES as u64 + 3 { memo.insert_group(Perm::Sop, TermId(k), rows(1), 1000) }
        assert_eq!(memo.1.lock().unwrap().len(), GROUP_CACHE_ENTRIES);
        // A group larger than the limit evicts everything else and is kept alone (callers never insert one: they check
        // the limit first).
        memo.insert_group(Perm::Ops, TermId(99), rows(5), 5);
        assert_eq!(memo.1.lock().unwrap().len(), 1);
    }
}
