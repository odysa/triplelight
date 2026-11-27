//! Basic-graph-pattern evaluation over flat row buffers (`w` IDs per row, no per-row allocation), parallelized
//! morsel-style: large inputs are split into chunks that rayon workers extend independently.
mod flat;
#[allow(dead_code)] mod table_plan; // automatic selection awaits calibrated residual costs
use super::{storage_failure, UNBOUND};
use super::plan_trace::Trace;
use super::pattern::{vars_of, Slot};
use super::range::{ObjRange, Ranges};
use crate::model::{Perm, TermId, TripleBatch};
use crate::storage::Snapshot;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use triplelight_sparql::algebra::{GraphPattern, OrderExpression, TriplePattern};

/// Identify output-only leaves of a SELECT over one filtered BGP.
pub(super) struct ProjectedBgp {
    patterns: usize,
    outputs: Vec<String>,
}

impl ProjectedBgp {
    pub(super) fn from_select(mut pattern: &GraphPattern) -> Option<Self> {
        let mut outputs = Vec::new();
        loop {
            pattern = match pattern {
                GraphPattern::Project { inner, variables } => {
                    outputs.extend(variables.iter().map(|v| v.as_str().to_owned()));
                    inner
                }
                GraphPattern::OrderBy { inner, expression } => {
                    for key in expression {
                        let (OrderExpression::Asc(e) | OrderExpression::Desc(e)) = key;
                        if super::impure(e) {
                            return None;
                        }
                        super::walk_expr(e, &mut outputs);
                    }
                    inner
                }
                GraphPattern::Distinct { inner } => inner,
                // SPARQL leaves the order of tied solutions unspecified. This rule may
                // change which tied rows survive a slice with ORDER BY.
                GraphPattern::Slice { inner, .. }
                    if matches!(super::range::slice_input(inner), GraphPattern::OrderBy { .. }) => inner,
                GraphPattern::Filter { inner, expr } => {
                    let GraphPattern::Bgp { patterns } = inner.as_ref() else { return None };
                    if patterns.len() < 2 || super::impure(expr) {
                        return None;
                    }
                    #[cfg(feature = "rdf-12")]
                    if patterns.iter().any(super::triple_term::has_triple_term) {
                        return None;
                    }
                    let filter_vars = super::expr_vars(expr);
                    outputs.retain(|v| !filter_vars.contains(v));
                    return (!outputs.is_empty()).then_some(Self { patterns: patterns.as_ptr() as usize, outputs });
                }
                // Other consumers, including aggregates and unordered slices, keep their order.
                _ => return None,
            };
        }
    }

    pub(super) fn matches(&self, patterns: &[TriplePattern]) -> bool {
        patterns.as_ptr() as usize == self.patterns
    }

    pub(super) fn leaves(&self, patterns: &[[Slot; 3]], lookup: impl Fn(&str) -> Option<usize>) -> Vec<usize> {
        patterns.iter().enumerate().filter_map(|(i, p)| {
            let [Slot::Var(_), Slot::Const(Some(_)), Slot::Var(object)] = *p else { return None };
            let output = self.outputs.iter().any(|name| lookup(name) == Some(object));
            let sole_occurrence = || patterns.iter().flatten()
                .filter(|slot| matches!(slot, Slot::Var(v) if *v == object)).take(2).count() == 1;
            (output && sole_occurrence()).then_some(i)
        }).collect()
    }
}

/// Rows per parallel work unit.
const MORSEL: usize = 1024;
/// Input rows from which a probe join sorts its rows by the probe key first (`Bgp::sorted_probe`).
const SORTED_PROBE_MIN_ROWS: usize = 16 * 1024;
/// Cost of one index probe (a seek plus a short range read) in scanned rows, for choosing between one probe per input
/// row and one scan of the whole range (`extend_hash`'s semi-join).
const PROBE_ROWS: u64 = 20;
/// Input rows from which a probe join over key-sorted rows merges with one positioned scan per morsel instead
/// (`Bgp::extend_merge`), and the rows per such morsel.
const MERGE_MIN_ROWS: usize = 4096;
const MERGE_MORSEL: usize = 8192;

/// The pattern shape `extend_sort_merge` handles: exactly one constant, one variable already bound (the merge key)
/// and one free variable, as `(constant, key, free)` slot positions. `bound` says whether a variable is bound in
/// the input. Shared with the wide-table selector's index cost estimate (#140), so the two cannot drift apart.
pub(crate) fn sort_merge_shape(pat: &[Slot; 3], bound: impl Fn(usize) -> bool) -> Option<(usize, usize, usize)> {
    let (mut cpos, mut kpos, mut fpos) = (None, None, None);
    for (i, s) in pat.iter().enumerate() {
        match s {
            Slot::Const(Some(_)) => { if cpos.is_some() { return None } cpos = Some(i) }
            Slot::Const(None) => return None,
            Slot::Var(v) if bound(*v) => { if kpos.is_some() { return None } kpos = Some(i) }
            Slot::Var(_) => { if fpos.is_some() { return None } fpos = Some(i) }
        }
    }
    let (cpos, kpos, fpos) = (cpos?, kpos?, fpos?);
    let (Slot::Var(kv), Slot::Var(fv)) = (pat[kpos], pat[fpos]) else { return None };
    (kv != fv).then_some((cpos, kpos, fpos))
}

/// Whether a step of that shape is a sort-merge rather than a hash join: in a BGP whose output order is free
/// (`order_free`; today the executor passes "a range was pushed", which implies it), with `n` input rows and `total` rows under the constant, and exactly in the window where
/// `extend_hash` would build a semi-join (not a whole-range table, and within `PROBE_ROWS` per input row).
/// Shared with the wide-table selector's index cost estimate (#140).
pub(crate) fn sort_merge_applies(order_free: bool, n: u64, total: u64) -> bool {
    order_free
        && (MERGE_MIN_ROWS as u64..=u32::MAX as u64).contains(&n)
        && !(total <= 2 * n && total <= 1 << 24)
        && total <= PROBE_ROWS * n
}

#[cfg(test)]
mod sort_merge_window {
    use super::*;
    #[test]
    fn the_window_is_extend_hash_s_semi_join_case_behind_a_range() {
        let n = 1u64 << 20;
        assert!(sort_merge_applies(true, n, 3 * n), "a semi-join behind a range sort-merges");
        assert!(!sort_merge_applies(false, n, 3 * n), "the BGP's order is not free");
        assert!(!sort_merge_applies(true, n, 2 * n), "a whole-range table: extend_hash builds it");
        assert!(!sort_merge_applies(true, n, PROBE_ROWS * n + 1), "past PROBE_ROWS per row: probes");
        assert!(!sort_merge_applies(true, MERGE_MIN_ROWS as u64 - 1, 1 << 30), "too few rows to pay for a sort");
        assert!(sort_merge_applies(true, 1 << 24, (1 << 24) + 1), "within 2n but past the whole-table cap: a semi-join");
    }
    #[test]
    fn the_shape_is_one_constant_one_bound_key_one_free_variable() {
        let c = Slot::Const(Some(TermId(7)));
        assert_eq!(sort_merge_shape(&[Slot::Var(0), c, Slot::Var(1)], |v| v == 0), Some((1, 0, 2)));
        assert_eq!(sort_merge_shape(&[Slot::Var(0), c, Slot::Var(1)], |_| true), None, "two bound variables");
        assert_eq!(sort_merge_shape(&[Slot::Var(0), c, Slot::Var(0)], |v| v == 0), None, "a repeated variable");
        assert_eq!(sort_merge_shape(&[Slot::Var(0), Slot::Const(None), Slot::Var(1)], |v| v == 0), None, "an absent constant");
    }
}

/// `intersect_count`: values of the smallest list per window, split across workers from this many on.
const PAR_LIST: usize = 32_768;
/// `intersect_count`: values per worker task.
const PAR_LIST_CHUNK: usize = 4096;

pub(super) struct Bgp<'a, S: Snapshot + ?Sized> {
    pub snap: &'a S,
    /// Row width (number of query variables).
    pub w: usize,
    /// Patterns whose object scan is restricted by a pushed-down range filter (`range.rs`), by (predicate, object
    /// variable). Used only when the pattern's subject and object are unbound.
    pub ranges: Option<&'a Ranges>,
    /// Sort-then-probe (`Options::sorted_probe`): see `extend_sorted`.
    pub sorted_probe: bool,
    /// Checked every `CHECK_ROWS` input rows of a probe or merge join (`Options::interrupt`).
    pub interrupt: Option<&'a super::Interrupt>,
    /// The query's working-memory budget: a join step's input and output rows must fit in what is left of it.
    pub budget: Option<&'a super::MemoryBudget>,
}

/// Input rows between two interrupt checks inside a probe join (each row may extend into many).
const CHECK_ROWS: usize = 64;
/// Bytes of a hash table (std's SwissTable) allocated for `n` entries of `slot` bytes: a power-of-two bucket count
/// keeping the table at most 7/8 full, one control byte per bucket plus one 16-byte group. The hash step allocates its
/// tables at this size up front, so they never grow (no old and new table alive together).
fn table_bytes(n: u64, slot: u64) -> u64 {
    let buckets = if n < 4 { 4 } else if n < 8 { 8 } else { (n * 8).div_ceil(7).next_power_of_two() };
    buckets * (slot + 1) + 16
}
/// Slot bytes of the hash step's tables: a `FastSet<TermId>` key, an `IdMap<(u32, u32)>` key and value.
const HASH_SET_SLOT: u64 = 8;
const HASH_MAP_SLOT: u64 = 16;

/// Appends `row` extended by one matching index row, where `value(k)` is that row's value at key position `k`.
/// A variable bound by the row must equal the index value (repeated variable); otherwise the candidate is dropped.
#[inline(always)]
fn push_extension(out: &mut Vec<TermId>, row: &[TermId], pat: &[Slot; 3], order: &[usize; 3], n: usize, value: impl Fn(usize) -> TermId) {
    let start = out.len();
    out.extend_from_slice(row);
    for (k, &pos) in order.iter().enumerate().skip(n) {
        if let Slot::Var(v) = pat[pos] {
            let val = value(k);
            let cell = &mut out[start + v];
            if *cell == UNBOUND { *cell = val } else if *cell != val { out.truncate(start); return }
        }
    }
}

impl<'a, S: Snapshot + ?Sized> Bgp<'a, S> {
    fn check(&self) { if let Some(i) = self.interrupt { i.check() } }
    fn count(&self, perm: Perm, prefix: &[TermId]) -> u64 { self.snap.count(perm, prefix).unwrap_or_else(|e| storage_failure(e)) }
    fn seek(&self, perm: Perm, prefix: &[TermId], t: TermId) -> Option<TermId> {
        self.snap.seek(perm, prefix, t).unwrap_or_else(|e| storage_failure(e))
    }

    /// Variables bound in every row of `rows`.
    fn bound_in_every_row(&self, rows: &[TermId]) -> HashSet<usize> {
        let w = self.w;
        (0..w).filter(|&i| rows.chunks(w).all(|r| r[i] != UNBOUND)).collect()
    }

    /// One join step shared by both evaluation orders: extend `rows` by pattern `pat`, mark its variables bound, then
    /// let `post` apply whatever has become evaluable. With a plan trace (`plan_trace`), records the step as pattern `pi`.
    fn join_step(&self, pat: &[Slot; 3], pi: usize, rows: &[TermId], bound: &mut HashSet<usize>, post: &mut dyn FnMut(&mut Vec<TermId>, &HashSet<usize>), trace: Option<&mut Trace>) -> Vec<TermId> {
        let t = trace.is_some().then(std::time::Instant::now);
        let (mut out, access) = self.extend_via(pat, rows);
        let extended = out.len();
        let step_bytes = (rows.len() + out.len()) as u64 * 8;
        if let Some(b) = self.budget && step_bytes > b.available() {
            super::limits::over_budget(super::OverBudget { requested: step_bytes, available: b.available() })
        }
        for s in pat { if let Slot::Var(v) = s { bound.insert(*v); } }
        post(&mut out, bound);
        if let (Some(tr), Some(t)) = (trace, t) { let w = self.w.max(1); tr.step(Some(pi), access, rows.len() / w, extended / w, out.len() / w, t) }
        out
    }

    /// Greedy order with exact index counts, evaluated by index nested-loop joins.
    /// `post` runs after every join step with the set of variables bound so far (used to apply filters as early as
    /// their variables are bound).
    pub fn eval_greedy(&self, pats: &[[Slot; 3]], leaves: &[usize], mut rows: Vec<TermId>, post: &mut dyn FnMut(&mut Vec<TermId>, &HashSet<usize>), mut trace: Option<&mut Trace>) -> Vec<TermId> {
        let w = self.w;
        if rows.is_empty() { return rows }
        let mut remaining: Vec<usize> = (0..pats.len()).collect();
        let mut bound = self.bound_in_every_row(&rows);
        while !remaining.is_empty() && !rows.is_empty() {
            let sample = rows[..w].to_vec();
            let is_bound_leaf = |pi: usize| {
                leaves.contains(&pi) && matches!(pats[pi][0], Slot::Var(v) if bound.contains(&v))
            };
            // A bound output-only leaf unlocks neither a filter nor another join. On
            // equal estimated fanout, let the other pattern reduce the input first.
            let (k, _, _) = remaining.iter().enumerate()
                .map(|(k, &pi)| (k, pi, self.estimate(&pats[pi], &bound, &sample)))
                .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap()
                    .then_with(|| is_bound_leaf(a.1).cmp(&is_bound_leaf(b.1)))).unwrap();
            let pi = remaining.remove(k);
            rows = self.join_step(&pats[pi], pi, &rows, &mut bound, post, trace.as_deref_mut());
        }
        rows
    }

    pub fn eval_order(&self, pats: &[[Slot; 3]], order: &[usize], mut rows: Vec<TermId>, post: &mut dyn FnMut(&mut Vec<TermId>, &HashSet<usize>), mut trace: Option<&mut Trace>) -> Vec<TermId> {
        let mut bound = if rows.is_empty() { HashSet::new() } else { self.bound_in_every_row(&rows) };
        for &pi in order {
            if rows.is_empty() { break }
            rows = self.join_step(&pats[pi], pi, &rows, &mut bound, post, trace.as_deref_mut());
        }
        rows
    }

    fn estimate(&self, pat: &[Slot; 3], bound: &HashSet<usize>, sample: &[TermId]) -> f64 {
        let fixed: [Option<TermId>; 3] = std::array::from_fn(|i| match pat[i] {
            Slot::Const(c) => c,
            Slot::Var(v) if bound.contains(&v) => Some(sample[v]),
            _ => None,
        });
        let c = match self.ranged(pat, |v| bound.contains(&v)) {
            Some(r) => r.rows as f64,
            None => { let (perm, prefix, n) = choose_perm(&fixed); self.count(perm, &prefix[..n]) as f64 }
        };
        let connected = pat.iter().any(|s| matches!(s, Slot::Var(v) if bound.contains(v)));
        if bound.is_empty() || connected { c } else { c * 1e6 } // avoid cross products
    }

    /// The pushed-down object range of `pat` if it has one and neither its subject nor its object is bound
    /// (`is_bound`); otherwise the pattern is read through its bound positions as usual.
    fn ranged(&self, pat: &[Slot; 3], is_bound: impl Fn(usize) -> bool) -> Option<&'a ObjRange> {
        let [Slot::Var(s), Slot::Const(Some(p)), Slot::Var(o)] = *pat else { return None };
        if is_bound(o) || is_bound(s) { return None }
        self.ranges?.get(&(p, o))
    }

    /// `pat` read through its pushed-down object range: POS with the predicate and each candidate object bound, so
    /// only triples whose object can pass the range filter are produced (the filter itself still runs afterwards).
    fn extend_range(&self, pat: &[Slot; 3], rows: &[TermId]) -> Option<Vec<TermId>> {
        let w = self.w;
        if w == 0 || rows.is_empty() { return None }
        let first = &rows[..w];
        let r = self.ranged(pat, |v| first[v] != UNBOUND)?;
        let [Slot::Var(s), Slot::Const(Some(p)), Slot::Var(o)] = *pat else { return None };
        if rows.chunks_exact(w).any(|row| row[o] != UNBOUND || row[s] != UNBOUND) { return None }
        // POS key order: predicate, object, subject.
        let order = Perm::Pos.order();
        let mut matches: Vec<[TermId; 3]> = Vec::with_capacity(r.rows.min(1 << 24) as usize);
        let mut keep = |b: TripleBatch, all: bool| {
            // `r.objects` is ascending, so membership is a binary search.
            matches.extend((0..b.len()).filter(|&i| all || r.objects.binary_search(&b.cols[1][i]).is_ok()).map(|i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]]));
        };
        // One probe per object, unless that costs more than one scan of the predicate (a dense range).
        if r.objects.len() as u64 * PROBE_ROWS > self.count(Perm::Pos, &[p]) {
            for b in self.snap.scan(Perm::Pos, &[p]) { keep(b.unwrap_or_else(|e| storage_failure(e)), false) }
        } else {
            for &obj in &r.objects {
                for b in self.snap.scan(Perm::Pos, &[p, obj]) { keep(b.unwrap_or_else(|e| storage_failure(e)), true) }
            }
        }
        let mut out = Vec::with_capacity(matches.len() * (rows.len() / w));
        for row in rows.chunks_exact(w) {
            for t in &matches { push_extension(&mut out, row, pat, &order, 1, |k| t[k]) }
        }
        Some(out)
    }

    /// Index nested-loop join of `rows` with one pattern: one index range read per input row.
    pub fn extend(&self, pat: &[Slot; 3], rows: &[TermId]) -> Vec<TermId> { self.extend_via(pat, rows).0 }

    /// `extend`, also naming the access path it took (for `plan_trace`).
    fn extend_via(&self, pat: &[Slot; 3], rows: &[TermId]) -> (Vec<TermId>, &'static str) {
        let w = self.w;
        if let Some(out) = self.extend_range(pat, rows) { return (out, "range") }
        if let Some(out) = self.extend_member_set(pat, rows) { return (out, "member-set") }
        if let Some(out) = self.extend_merge(pat, rows) { return (out, "merge") }
        if let Some(out) = self.extend_sort_merge(pat, rows) { return (out, "sort-merge") }
        if let Some(out) = self.extend_hash(pat, rows) { return (out, "hash") }
        if let Some(out) = self.extend_sorted(pat, rows) { return (out, "sorted") }
        if rows.len() / w.max(1) >= 2 * MORSEL {
            let parts: Vec<Vec<TermId>> = rows.par_chunks(w * MORSEL).map(|c| self.extend_seq(pat, c)).collect();
            (parts.concat(), "probe-par")
        } else {
            (self.extend_seq(pat, rows), "probe")
        }
    }

    /// Merge join for a pattern with one constant, one variable bound in every row and one free variable when the
    /// rows are sorted by the bound variable (e.g. `?review dc:date ?date` after a PSO scan of `?review bsbm:reviewFor
    /// ?product`) and the constant's range is no more than `PROBE_ROWS` times the input: each morsel of rows reads the
    /// permutation `(constant, key, free)` once, positioned at its first key (`Snapshot::scan_from`), and walks it
    /// alongside the rows. Per row, the values come in index order, as a probe's would, and rows keep their order, so
    /// the output equals the probe join's.
    fn extend_merge(&self, pat: &[Slot; 3], rows: &[TermId]) -> Option<Vec<TermId>> {
        let w = self.w;
        if w == 0 { return None }
        let n = rows.len() / w;
        if n < MERGE_MIN_ROWS { return None }
        let first = &rows[..w];
        let (mut cpos, mut kpos, mut fpos) = (None, None, None);
        for (i, s) in pat.iter().enumerate() {
            match s {
                Slot::Const(Some(_)) => { if cpos.is_some() { return None } cpos = Some(i) }
                Slot::Const(None) => return None,
                Slot::Var(v) if first[*v] != UNBOUND => { if kpos.is_some() { return None } kpos = Some(i) }
                Slot::Var(_) => { if fpos.is_some() { return None } fpos = Some(i) }
            }
        }
        let (cpos, kpos, fpos) = (cpos?, kpos?, fpos?);
        let (Slot::Const(Some(c)), Slot::Var(kv), Slot::Var(fv)) = (pat[cpos], pat[kpos], pat[fpos]) else { return None };
        if kv == fv { return None }
        if rows.chunks_exact(w).any(|r| r[kv] == UNBOUND || r[fv] != UNBOUND) { return None }
        if !rows.chunks_exact(w).map(|r| r[kv]).is_sorted() { return None }
        let perm = Perm::ALL.into_iter().find(|p| p.order() == [cpos, kpos, fpos])?;
        if self.count(perm, &[c]) > PROBE_ROWS * n as u64 { return None }
        let parts: Vec<Vec<TermId>> = rows.par_chunks(w * MERGE_MORSEL).map(|chunk| {
            let (lo, hi) = (chunk[kv], chunk[chunk.len() - w + kv]);
            let mut out = Vec::with_capacity(chunk.len());
            let mut scan = self.snap.scan_from(perm, &[c], lo);
            let (mut batch, mut at) = (TripleBatch::default(), 0usize);
            // The values of the current key, reused by consecutive rows with that key.
            let (mut key, mut vals): (Option<TermId>, Vec<TermId>) = (None, Vec::new());
            // The scan's next row (key, free value), or None at its end or past `hi`.
            let mut next = || -> Option<(TermId, TermId)> {
                while at >= batch.len() {
                    batch = scan.next()?.unwrap_or_else(|e| storage_failure(e));
                    at = 0;
                }
                let r = (batch.cols[1][at], batch.cols[2][at]);
                if r.0 > hi { return None }
                at += 1;
                Some(r)
            };
            let mut pending = next();
            self.check();
            for row in chunk.chunks_exact(w) {
                let k = row[kv];
                if key != Some(k) {
                    vals.clear();
                    while let Some((pk, _)) = pending && pk < k { pending = next() }
                    while let Some((pk, f)) = pending && pk == k { vals.push(f); pending = next() }
                    key = Some(k);
                }
                for &f in &vals {
                    let at = out.len() + fv;
                    out.extend_from_slice(row);
                    out[at] = f;
                }
            }
            out
        }).collect();
        Some(parts.concat())
    }

    /// Sort-merge join for the semi-join case of `extend_hash`: a BGP with a pushed-down range (so its solution order
    /// cannot reach the answers, `range::reorderable_bgps`), many rows arriving unsorted on the join key (e.g. a date
    /// range's reviews, products and offers in date order, joined to `?review bsbm:reviewFor ?product`), and a
    /// constant's range too large for the whole-range hash table but within `PROBE_ROWS` per row. Instead of a key
    /// set, a filtered scan into a hash table and one random lookup per row, the rows are sorted by the key and
    /// `extend_merge` walks the permutation `(constant, key, free)` beside them. The bag of solutions is the hash
    /// join's; only their order differs, which the pushed range already makes free.
    fn extend_sort_merge(&self, pat: &[Slot; 3], rows: &[TermId]) -> Option<Vec<TermId>> {
        let w = self.w;
        if w == 0 { return None }
        let n = rows.len() / w;
        // Cheap exits first: the full check below needs a scan of every row and an index count.
        if !sort_merge_applies(self.ranges.is_some(), n as u64, PROBE_ROWS * n as u64) { return None }
        let first = rows.get(..w)?;
        let (cpos, kpos, fpos) = sort_merge_shape(pat, |v| first[v] != UNBOUND)?;
        let (Slot::Const(Some(c)), Slot::Var(kv), Slot::Var(fv)) = (pat[cpos], pat[kpos], pat[fpos]) else { return None };
        if rows.chunks_exact(w).any(|r| r[kv] == UNBOUND || r[fv] != UNBOUND) { return None }
        let perm = Perm::ALL.into_iter().find(|p| p.order() == [cpos, kpos, fpos])?;
        if !sort_merge_applies(self.ranges.is_some(), n as u64, self.count(perm, &[c])) { return None }
        let mut order: Vec<u32> = (0..n as u32).collect();
        order.par_sort_unstable_by_key(|&i| rows[i as usize * w + kv]);
        let mut sorted = Vec::with_capacity(rows.len());
        for &i in &order { let i = i as usize; sorted.extend_from_slice(&rows[i * w..(i + 1) * w]) }
        drop(order);
        self.extend_merge(pat, &sorted)
    }

    /// Membership filter for a pattern with two constants and one variable bound in every row (e.g.
    /// `?reviewer bsbm:country <US>`): when the constants' range is no larger than the input, one scan builds the
    /// set of matching values and each row is a hash lookup instead of an index probe. Keeps row order.
    fn extend_member_set(&self, pat: &[Slot; 3], rows: &[TermId]) -> Option<Vec<TermId>> {
        let w = self.w;
        if w == 0 { return None }
        let n_rows = rows.len() / w;
        if n_rows < 1024 { return None }
        let vars: Vec<(usize, usize)> = pat.iter().enumerate().filter_map(|(i, s)| match s { Slot::Var(v) => Some((i, *v)), _ => None }).collect();
        let [(pos, v)] = vars.as_slice() else { return None };
        let mut fixed: [Option<TermId>; 3] = [None; 3];
        for (i, s) in pat.iter().enumerate() {
            if let Slot::Const(c) = s { fixed[i] = Some((*c)?) }
        }
        let (perm, prefix, n) = choose_perm(&fixed);
        if n != 2 || perm.order()[2] != *pos { return None }
        if rows.chunks_exact(w).any(|r| r[*v] == UNBOUND) { return None }
        let total = self.count(perm, &prefix[..2]);
        if total > n_rows as u64 || total > 1 << 24 { return None }
        let mut set: crate::id_hash::FastSet<TermId> = crate::id_hash::fast_set(total as usize);
        for b in self.snap.scan(perm, &prefix[..2]) {
            let b = b.unwrap_or_else(|e| storage_failure(e));
            set.extend(b.cols[2].iter().copied());
        }
        let mut out = Vec::with_capacity(rows.len());
        for r in rows.chunks_exact(w) { if set.contains(&r[*v]) { out.extend_from_slice(r) } }
        Some(out)
    }

    /// Hash join for a pattern with one constant, one variable bound in every row and one free variable (e.g.
    /// `?review rev:reviewer ?reviewer` for many reviews): when the constant's range is at most twice the input,
    /// one scan of the permutation `(constant, key, free)` builds key -> values, and each row is a lookup instead
    /// of an index probe. Per row, values come in the same (index) order as the probe join's range scan. In a BGP
    /// with a pushed-down range, a larger range is still scanned once, as a semi-join on the rows' keys, when that is
    /// cheaper than probing per row (`PROBE_ROWS`).
    fn extend_hash(&self, pat: &[Slot; 3], rows: &[TermId]) -> Option<Vec<TermId>> {
        let w = self.w;
        if w == 0 { return None }
        let n_rows = rows.len() / w;
        if n_rows < 1024 { return None }
        let first = &rows[..w];
        let (mut cpos, mut kpos, mut fpos) = (None, None, None);
        for (i, s) in pat.iter().enumerate() {
            match s {
                Slot::Const(Some(_)) => { if cpos.is_some() { return None } cpos = Some(i) }
                Slot::Const(None) => return None,
                Slot::Var(v) if first[*v] != UNBOUND => { if kpos.is_some() { return None } kpos = Some(i) }
                Slot::Var(_) => { if fpos.is_some() { return None } fpos = Some(i) }
            }
        }
        let (cpos, kpos, fpos) = (cpos?, kpos?, fpos?);
        let (Slot::Const(Some(c)), Slot::Var(kv), Slot::Var(fv)) = (pat[cpos], pat[kpos], pat[fpos]) else { return None };
        if kv == fv { return None }
        if rows.chunks_exact(w).any(|r| r[kv] == UNBOUND || r[fv] != UNBOUND) { return None }
        let perm = [Perm::Spo, Perm::Sop, Perm::Pso, Perm::Pos, Perm::Osp, Perm::Ops].into_iter().find(|p| p.order() == [cpos, kpos, fpos])?;
        let total = self.count(perm, &[c]);
        let whole = total <= 2 * n_rows as u64 && total <= 1 << 24;
        // Semi-join, only in a BGP with a pushed-down range (whose first pattern can leave many rows in object order,
        // where today's plan would have scanned both patterns): when one probe per row would cost more than one
        // scan of the constant's range, scan it once and keep only the keys some row has. Memory is the kept
        // entries, so the 2^24 cap of the whole-range table does not apply.
        let semi = !whole && self.ranges.is_some() && total <= PROBE_ROWS * n_rows as u64 && total <= u64::from(u32::MAX);
        if !whole && !semi { return None }
        // Working memory, charged to the query budget before it is built (#111): the semi-join's key set, the
        // key -> (start, len) map (one entry per distinct scanned key: up to `total` for the whole range, up to the
        // input's keys for the semi-join) and the values, each table sized with hash-table slack. The whole range's
        // values are exactly `total`; the semi-join keeps every value of a kept key, so its values are charged as
        // they grow, before each reallocation.
        let kept = if semi { total.min(n_rows as u64) } else { total };
        let fixed = if semi { table_bytes(n_rows as u64, HASH_SET_SLOT) } else { 0 } + table_bytes(kept, HASH_MAP_SLOT);
        let vals_charged = |cap: usize| fixed + cap as u64 * 8;
        let mut vals_cap = if semi { n_rows } else { total as usize };
        let mut held = self.charge(vals_charged(vals_cap));
        let grow = |held: &mut Option<super::limits::Reservation<'a>>, bytes: u64| if let (Some(r), Some(b)) = (held.as_mut(), self.budget) {
            r.try_resize(bytes).unwrap_or_else(|_| super::limits::over_budget(super::OverBudget { requested: bytes, available: b.available() + r.bytes() }));
        };
        let keys: Option<crate::id_hash::FastSet<TermId>> = semi.then(|| {
            let mut ks = crate::id_hash::FastSet::with_capacity_and_hasher(n_rows, Default::default());
            ks.extend(rows.chunks_exact(w).map(|r| r[kv]));
            ks
        });
        let mut vals: Vec<TermId> = Vec::with_capacity(vals_cap);
        let mut index: crate::id_hash::IdMap<(u32, u32)> = crate::id_hash::IdMap::with_capacity_and_hasher(kept as usize, Default::default());
        for b in self.snap.scan(perm, &[c]) {
            let b = b.unwrap_or_else(|e| storage_failure(e));
            for i in 0..b.len() {
                let (k, f) = (b.cols[1][i], b.cols[2][i]);
                if keys.as_ref().is_some_and(|ks| !ks.contains(&k)) { continue }
                let e = index.entry(k).or_insert((vals.len() as u32, 0));
                e.1 += 1;
                if vals.len() == vals_cap {
                    // A reallocation may move the values: old and new buffers are both alive until it returns.
                    let next = (vals_cap * 2).max(1024);
                    grow(&mut held, vals_charged(vals_cap) + next as u64 * 8);
                    vals.reserve_exact(next - vals.len());
                    vals_cap = next;
                    grow(&mut held, vals_charged(vals_cap));
                }
                vals.push(f);
            }
        }
        drop(keys);
        // The output's exact size first, so it is allocated (and charged) once instead of growing by doubling, which
        // briefly holds the old and new buffers together.
        // One probe per row: each row's (start, len) is kept (8 B a row) so the emit loop does not look it up again.
        grow(&mut held, vals_charged(vals_cap) + n_rows as u64 * 8);
        let spans: Vec<(u32, u32)> = rows.chunks_exact(w).map(|r| index.get(&r[kv]).copied().unwrap_or((0, 0))).collect();
        let extensions: usize = spans.iter().map(|&(_, len)| len as usize).sum();
        grow(&mut held, vals_charged(vals_cap) + n_rows as u64 * 8 + extensions as u64 * w as u64 * 8);
        drop(index);
        let mut out = Vec::with_capacity(extensions * w);
        for (r, &(start, len)) in rows.chunks_exact(w).zip(&spans) {
            for &f in &vals[start as usize..(start + len) as usize] {
                out.extend_from_slice(r);
                let at = out.len() - w;
                out[at + fv] = f;
            }
        }
        drop(held);
        Some(out)
    }

    /// Charges `bytes` of a join step's working memory to the query budget, or ends the query with `ResourceLimit`.
    fn charge(&self, bytes: u64) -> Option<super::limits::Reservation<'a>> {
        self.budget.map(|b| b.try_charge(bytes).unwrap_or_else(|e| super::limits::over_budget(e)))
    }

    /// Sort-then-probe: a probe join of many rows (at least `SORTED_PROBE_MIN_ROWS`) whose pattern has exactly one
    /// variable bound (the key, bound in every row) probes the rows in key order, so consecutive probes read nearby
    /// index blocks instead of jumping across the index (e.g. reviews arriving in product order, probed by review
    /// in SPO). The output is put back in input-row order, each row's extensions in index order as `extend_seq`
    /// gives them, so the result is identical to the unsorted probe join. The cost is a stable sort of row indices,
    /// one copy of the input and one gather of the output. `None` (the caller probes in input order) when off, the
    /// input is small, has no single bound key, or is already in key order.
    fn extend_sorted(&self, pat: &[Slot; 3], rows: &[TermId]) -> Option<Vec<TermId>> {
        let w = self.w;
        if !self.sorted_probe || w == 0 { return None }
        let n = rows.len() / w;
        if n < SORTED_PROBE_MIN_ROWS || n > u32::MAX as usize { return None }
        let first = &rows[..w];
        let mut key = None;
        for s in pat {
            if let Slot::Var(v) = *s && first[v] != UNBOUND {
                if key.is_some_and(|k| k != v) { return None }
                key = Some(v);
            }
        }
        let kv = key?;
        if rows.chunks_exact(w).any(|r| r[kv] == UNBOUND) { return None }
        if rows.chunks_exact(w).map(|r| r[kv]).is_sorted() { return None }
        let mut order: Vec<u32> = (0..n as u32).collect();
        order.par_sort_by_key(|&i| rows[i as usize * w + kv]);
        let mut sorted = Vec::with_capacity(rows.len());
        for &i in &order {
            let i = i as usize;
            sorted.extend_from_slice(&rows[i * w..(i + 1) * w]);
        }
        // Per morsel of sorted rows: its output and where each row's extensions end in it.
        let parts: Vec<(Vec<TermId>, Vec<usize>)> = sorted.par_chunks(w * MORSEL).map(|chunk| {
            let mut out = Vec::with_capacity(chunk.len());
            let mut ends = Vec::with_capacity(chunk.len() / w);
            self.check();
            for row in chunk.chunks_exact(w) {
                self.probe_row(pat, row, &mut out);
                ends.push(out.len());
            }
            (out, ends)
        }).collect();
        // Input row -> (morsel, start, end) of its extensions, then gather in input order.
        let mut segment = vec![(0u32, 0usize, 0usize); n];
        let mut j = 0;
        for (p, (_, ends)) in parts.iter().enumerate() {
            let mut start = 0;
            for &end in ends {
                segment[order[j] as usize] = (p as u32, start, end);
                start = end;
                j += 1;
            }
        }
        let mut out = Vec::with_capacity(parts.iter().map(|p| p.0.len()).sum());
        for &(p, start, end) in &segment { out.extend_from_slice(&parts[p as usize].0[start..end]) }
        Some(out)
    }

    fn extend_seq(&self, pat: &[Slot; 3], rows: &[TermId]) -> Vec<TermId> {
        let w = self.w;
        let mut out = Vec::with_capacity(rows.len());
        for (i, row) in rows.chunks_exact(w.max(1)).enumerate() {
            if i.is_multiple_of(CHECK_ROWS) { self.check() }
            self.probe_row(pat, &row[..w], &mut out)
        }
        out
    }

    /// Appends `row`'s extensions by `pat` (one index probe), in index order.
    fn probe_row(&self, pat: &[Slot; 3], row: &[TermId], out: &mut Vec<TermId>) {
        let fixed: [Option<TermId>; 3] = std::array::from_fn(|i| match pat[i] {
            Slot::Const(c) => c,
            Slot::Var(v) => (row[v] != UNBOUND).then_some(row[v]),
        });
        let (perm, prefix, n) = choose_perm(&fixed);
        if n == 3 {
            // Fully bound: a membership probe, no scan.
            if self.count(perm, &prefix) > 0 { out.extend_from_slice(row) }
            return;
        }
        let order = perm.order();
        // Compact layout: rows without their first key column; `t[k - 1]` is key position `k` (k >= n >= 1).
        if let Some(tails) = self.snap.matching_tail(perm, &prefix[..n]) {
            for t in tails { push_extension(out, row, pat, &order, n, |k| t[k - 1]); }
            return;
        }
        if let Some(rows) = self.snap.matching_rows(perm, &prefix[..n]) {
            for t in rows { push_extension(out, row, pat, &order, n, |k| t[k]); }
            return;
        }
        // Which key columns bind new variables, and which must equal an already-bound value (repeated variable).
        for batch in self.snap.scan(perm, &prefix[..n]) {
            let batch = batch.unwrap_or_else(|e| storage_failure(e));
            #[allow(clippy::needless_range_loop)] // columnar batch: one row across several columns
            for i in 0..batch.len() { push_extension(out, row, pat, &order, n, |k| batch.cols[k][i]); }
        }
    }

    /// Exact match count of each pattern's constant prefix (its candidate rows before any join).
    fn const_counts<'b>(&'b self, pats: &'b [[Slot; 3]]) -> impl Iterator<Item = u64> + 'b {
        pats.iter().map(move |p| {
            let fixed: [Option<TermId>; 3] = std::array::from_fn(|i| if let Slot::Const(c) = p[i] { c } else { None });
            let (perm, prefix, n) = choose_perm(&fixed);
            self.count(perm, &prefix[..n])
        })
    }

    /// Leapfrog triejoin (Veldhuizen, ICDT'14), the worst-case-optimal join: bind one variable at a time,
    /// intersecting by index seeks the sorted value lists of every pattern containing it. Each pattern reads the
    /// permutation keyed (its constants, then its variables in the global order). The first variable's values are
    /// split across rayon workers.
    pub fn leapfrog(&self, pats: &[[Slot; 3]], row: &[TermId], given: Option<Vec<usize>>) -> Vec<TermId> {
        let counts: Vec<_> = self.const_counts(pats).collect();
        if counts.contains(&0) { return Vec::new() }
        let (order, plans) = lf_plan(pats, &counts, given);
        let mut row = row.to_vec();
        let first = self.intersect(&plans, order[0], &row);
        if first.len() >= 64 {
            let parts: Vec<Vec<TermId>> = first.par_chunks(16).map(|vals| {
                let mut r = row.clone();
                let mut out = Vec::new();
                for &x in vals { r[order[0]] = x; self.lf_depth(&plans, &order, 1, &mut r, &mut out); }
                out
            }).collect();
            parts.concat()
        } else {
            let mut out = Vec::new();
            for x in first { row[order[0]] = x; self.lf_depth(&plans, &order, 1, &mut row, &mut out); }
            out
        }
    }

    /// Keep the ordinary binary join order, but sum the final range counts
    /// rather than allocating its output rows. Preserve morsel parallelism.
    pub fn count_binary(&self,pats:&[[Slot;3]],order:Option<&[usize]>)->u64 {
        if pats.is_empty(){return 1;}
        let mut seen=HashSet::new();
        if pats.iter().all(|p|p.iter().filter_map(|s|if let Slot::Var(v)=s{Some(*v)}else{None}).all(|v|seen.insert(v))) {
            return self.const_counts(pats).fold(1, checked_count_product);
        }
        let mut rows=vec![UNBOUND;self.w];let mut bound=HashSet::new();
        let mut remaining:Vec<_>=order.map_or_else(||(0..pats.len()).collect(),|o|o.to_vec());
        while remaining.len()>1 && !rows.is_empty(){
            let k=if order.is_some(){0}else{
                remaining.iter().enumerate().map(|(k,&pi)|(k,self.estimate(&pats[pi],&bound,&rows[..self.w]))).min_by(|a,b|a.1.partial_cmp(&b.1).unwrap()).unwrap().0
            };
            let pi=remaining.remove(k);rows=self.extend(&pats[pi],&rows);
            for s in &pats[pi]{if let Slot::Var(v)=s{bound.insert(*v);}}
        }
        if rows.is_empty(){return 0;}
        let pat=&pats[remaining[0]];
        let part=|ids:&[TermId]| {
            ids.chunks_exact(self.w).map(|row| {
                let fixed=std::array::from_fn(|i|match pat[i]{Slot::Const(c)=>c,Slot::Var(v)=>(row[v]!=UNBOUND).then_some(row[v])});
                let (perm,prefix,n)=choose_perm(&fixed);self.count(perm,&prefix[..n])
            }).fold(0,checked_count)
        };
        if rows.len()/self.w>=2*MORSEL { rows.par_chunks(self.w*MORSEL).map(part).reduce(||0,checked_count) }else{part(&rows)}
    }

    /// Count complete BGP bindings without allocating result rows. The final
    /// variable contributes an intersection size, or an exact prefix count when
    /// only one relation contains it. Callers reject repeated-variable patterns.
    pub fn count_leapfrog(&self, pats: &[[Slot; 3]], given: Option<Vec<usize>>) -> u64 {
        let counts: Vec<_> = self.const_counts(pats).collect();
        if counts.contains(&0) { return 0; }
        if pats.len() == 1 { return counts[0]; }
        // Disconnected BGP components are independent factors. Counting their
        // product avoids walking an otherwise enormous Cartesian product.
        let mut components: Vec<Vec<usize>> = Vec::new();
        let mut remaining: Vec<usize> = (0..pats.len()).collect();
        while let Some(first) = remaining.pop() {
            let mut group = vec![first];
            let mut variables: HashSet<usize> = pats[first].iter().filter_map(|s| if let Slot::Var(v)=s {Some(*v)} else {None}).collect();
            while let Some(k) = remaining.iter().position(|&i| pats[i].iter().any(|s| matches!(s, Slot::Var(v) if variables.contains(v)))) {
                let i = remaining.remove(k); group.push(i);
                variables.extend(pats[i].iter().filter_map(|s| if let Slot::Var(v)=s {Some(*v)} else {None}));
            }
            components.push(group);
        }
        if components.len() > 1 {
            return components.into_iter().map(|indices| {
                let part: Vec<_> = indices.into_iter().map(|i|pats[i]).collect();
                self.count_leapfrog(&part,None)
            }).fold(1, checked_count_product);
        }
        if pats.iter().all(|p| vars_of(p).is_empty()) { return 1; }
        let (order, plans) = lf_plan(pats, &counts, given);
        if order.len()==1 { return self.lf_count_depth(&plans,&order,0,&mut vec![UNBOUND;self.w]); }
        let row=vec![UNBOUND;self.w];
        let first=self.intersect(&plans,order[0],&row);
        let part=|values:&[TermId]| {
            let mut row=row.clone();let mut sum=0;
            for &value in values { row[order[0]]=value;sum=checked_count(sum,self.lf_count_depth(&plans,&order,1,&mut row)); }
            sum
        };
        if first.len()>=64 { first.par_chunks(16).map(part).reduce(||0,checked_count) } else { part(&first) }
    }

    /// Solutions of a star BGP (`pattern::star_center`), and how many distinct center values they have, without
    /// enumerating the leaves. The candidates are the leapfrog intersection of the most selective pattern and every
    /// pattern with no leaf; each pattern with leaves is probed with one range count per candidate (zero drops it), and
    /// a candidate contributes the product of those counts. Split across rayon workers
    /// like `count_leapfrog`.
    pub fn count_star(&self, pats: &[[Slot; 3]], center: usize) -> (u64, u64) {
        let mut counted: Vec<(u64, [Slot; 3])> = self.const_counts(pats).zip(pats.iter().copied()).collect();
        if counted.iter().any(|&(n, _)| n == 0) { return (0, 0) }
        // Candidates from the most selective pattern; the next most selective probes first, so misses stop early.
        counted.sort_by_key(|&(n, _)| n);
        let (counts, pats): (Vec<u64>, Vec<[Slot; 3]>) = counted.into_iter().unzip();
        let mut order = vec![center];
        for p in &pats { order.extend(vars_of(p).into_iter().filter(|&v| v != center)) }
        let (_, plans) = lf_plan(&pats, &counts, Some(order));
        // Each plan is keyed constants, then the center, then its own leaves: the center's count prefix.
        let prefixes: Vec<(Perm, [TermId; 3], usize)> = plans.iter().map(|(perm, key)| {
            let k = key.iter().position(|s| matches!(s, Slot::Var(v) if *v == center)).unwrap();
            let mut prefix = [UNBOUND; 3];
            for (j, s) in key[..k].iter().enumerate() { if let Slot::Const(id) = s { prefix[j] = id.unwrap() } }
            (*perm, prefix, k)
        }).collect();
        // A pattern whose only variable is the center (`?c p o`) is a sorted list of exactly the centers it admits:
        // the leapfrog intersects those, skipping ahead in each, and only the patterns with leaves are probed.
        let (lists, probes): (Vec<usize>, Vec<usize>) = (0..plans.len()).partition(|&i| prefixes[i].2 == 2);
        // The most selective pattern leads the intersection too, leaves or not (it is then also probed for its count).
        let lists = if lists.contains(&0) { lists } else { std::iter::once(0).chain(lists).collect() };
        // Only lists (`?c p o` patterns): the answer is the intersection's size, and `lists[0]` is the smallest by its
        // exact count (the plans are in count order).
        if probes.is_empty() {
            let n = self.intersect_count(&lists.iter().map(|&i| (prefixes[i].0, prefixes[i].1)).collect::<Vec<_>>());
            return (n, n);
        }
        let probes: Vec<_> = probes.into_iter().map(|i| prefixes[i]).collect();
        let lists: Vec<_> = lists.into_iter().map(|i| plans[i]).collect();
        // Candidates ascend, so every pattern's seeks and counts move forward through its index: each pattern gets a
        // probe (`Snapshot::probe`) that resumes from where its previous search landed.
        let mut candidates = Vec::new();
        {
            let mut seekers: Vec<_> = lists.iter().map(|&(perm, _)| self.snap.probe(perm)).collect();
            self.leap_with(&lists, center, &vec![UNBOUND; self.w], |i, _, prefix, x| {
                seekers[i].seek(prefix, x).unwrap_or_else(|e| storage_failure(e))
            }, |x| candidates.push(x));
        }
        let part = |values: &[TermId]| {
            let (mut sum, mut centers) = (0, 0);
            let mut counters: Vec<_> = probes.iter().map(|&(perm, _, _)| self.snap.probe(perm)).collect();
            'next: for &c in values {
                let mut product = 1;
                for (j, &(_, mut prefix, k)) in probes.iter().enumerate() {
                    prefix[k] = c;
                    let n = counters[j].count(&prefix[..=k]).unwrap_or_else(|e| storage_failure(e));
                    if n == 0 { continue 'next }
                    product = checked_count_product(product, n);
                }
                (sum, centers) = (checked_count(sum, product), centers + 1);
            }
            (sum, centers)
        };
        let add = |a: (u64, u64), b: (u64, u64)| (checked_count(a.0, b.0), a.1 + b.1);
        if candidates.len() >= 64 { candidates.par_chunks(256).map(part).reduce(|| (0, 0), add) } else { part(&candidates) }
    }

    /// How many values every sorted center list `(perm, [constant, constant, _])` holds, the first being the smallest.
    /// The first list is read by a scan, so it costs no searches, and each value is sought in the other lists through
    /// a cursor (its targets ascend). From `PAR_LIST` values on, a window is split across rayon workers, each with its
    /// own cursors; below that one thread does it, since the fork would cost more than a sub-millisecond count.
    fn intersect_count(&self, lists: &[(Perm, [TermId; 3])]) -> u64 {
        let ((perm, first), rest) = (lists[0], &lists[1..]);
        let count = |values: &[TermId]| -> u64 {
            let mut seekers: Vec<_> = rest.iter().map(|&(perm, _)| self.snap.probe(perm)).collect();
            let mut n = 0;
            'next: for &c in values {
                for (seeker, (_, prefix)) in seekers.iter_mut().zip(rest) {
                    if seeker.seek(&prefix[..2], c).unwrap_or_else(|e| storage_failure(e)) != Some(c) { continue 'next }
                }
                n += 1;
            }
            n
        };
        let mut window: Vec<TermId> = Vec::new();
        let flush = |window: &mut Vec<TermId>| -> u64 {
            self.check();
            let n = if window.len() >= PAR_LIST { window.par_chunks(PAR_LIST_CHUNK).map(count).sum() } else { count(window) };
            window.clear();
            n
        };
        let mut total = 0;
        for batch in self.snap.scan(perm, &first[..2]) {
            window.extend_from_slice(&batch.unwrap_or_else(|e| storage_failure(e)).cols[2]);
            if window.len() >= PAR_LIST { total += flush(&mut window) }
        }
        total + flush(&mut window)
    }

    fn lf_count_depth(&self, plans: &[(Perm, [Slot; 3])], order: &[usize], depth: usize, row: &mut Vec<TermId>) -> u64 {
        let v = order[depth];
        if depth + 1 == order.len() {
            let mut sources = plans.iter().filter(|(_, key)| key.iter().any(|s| matches!(s, Slot::Var(x) if *x == v)));
            let first = sources.next().expect("count variable has an index source");
            if sources.next().is_none() {
                let (perm, key) = first;
                let k = key.iter().position(|s| matches!(s, Slot::Var(x) if *x == v)).unwrap();
                let prefix: Vec<_> = key[..k].iter().map(|s| match s { Slot::Const(c) => c.unwrap(), Slot::Var(x) => row[*x] }).collect();
                return self.count(*perm, &prefix);
            }
            let mut n = 0u64;
            self.leap(plans, v, row, |_| { n = checked_count(n, 1); });
            return n;
        }
        let mut total = 0;
        let prefix = row.clone();
        self.leap(plans, v, &prefix, |value| {
            row[v] = value;
            total = checked_count(total, self.lf_count_depth(plans, order, depth + 1, row));
        });
        row[v] = UNBOUND;
        total
    }

    /// All values of `v` on which every pattern containing it agrees, given the bound prefix in `row`.
    fn intersect(&self, plans: &[(Perm, [Slot; 3])], v: usize, row: &[TermId]) -> Vec<TermId> {
        let mut out = Vec::new();
        self.leap(plans, v, row, |x| out.push(x));
        out
    }

    fn leap(&self, plans: &[(Perm, [Slot; 3])], v: usize, row: &[TermId], emit: impl FnMut(TermId)) {
        self.leap_with(plans, v, row, |_, perm, prefix, x| self.seek(perm, prefix, x), emit)
    }

    /// `leap`, seeking through `seek(i, perm, prefix, target)`, where `i` numbers the plans that contain `v` in order;
    /// each one's seek targets ascend.
    fn leap_with(&self, plans: &[(Perm, [Slot; 3])], v: usize, row: &[TermId], mut seek: impl FnMut(usize, Perm, &[TermId], TermId) -> Option<TermId>, mut emit: impl FnMut(TermId)) {
        let mut its: Vec<(Perm, [TermId; 3], usize)> = Vec::with_capacity(plans.len());
        for (perm, key) in plans {
            let Some(k) = key.iter().position(|s| matches!(s, Slot::Var(x) if *x == v)) else { continue };
            let mut prefix = [UNBOUND; 3];
            for (j, s) in key[..k].iter().enumerate() {
                prefix[j] = match s { Slot::Const(c) => c.unwrap(), Slot::Var(x) => row[*x] };
            }
            its.push((*perm, prefix, k));
        }
        let mut x = TermId(1);
        'outer: loop {
            let (mut agreed, mut i) = (0, 0);
            while agreed < its.len() {
                let (perm, prefix, k) = &its[i];
                let Some(y) = seek(i, *perm, &prefix[..*k], x) else { break 'outer };
                if y == x { agreed += 1 } else { x = y; agreed = 1 }
                i = (i + 1) % its.len();
            }
            emit(x);
            if x.0 == u64::MAX { break }
            x = TermId(x.0 + 1);
        }
    }

    fn lf_depth(&self, plans: &[(Perm, [Slot; 3])], order: &[usize], d: usize, row: &mut Vec<TermId>, out: &mut Vec<TermId>) {
        if d == order.len() { out.extend_from_slice(row); return }
        let v = order[d];
        let vals = self.intersect(plans, v, row);
        for x in vals {
            row[v] = x;
            self.lf_depth(plans, order, d + 1, row, out);
        }
        row[v] = UNBOUND;
    }
}

/// Permutation whose leading key components are exactly the fixed positions; returns (perm, key prefix, length).
pub(super) fn choose_perm(fixed: &[Option<TermId>; 3]) -> (Perm, [TermId; 3], usize) {
    let perm = match [fixed[0].is_some(), fixed[1].is_some(), fixed[2].is_some()] {
        [true, true, _] => Perm::Spo,
        [true, false, true] => Perm::Sop,
        [true, false, false] => Perm::Spo,
        [false, true, true] => Perm::Pos,
        [false, true, false] => Perm::Pso,
        [false, false, true] => Perm::Osp,
        [false, false, false] => Perm::Spo,
    };
    let mut prefix = [UNBOUND; 3];
    let mut n = 0;
    for &p in &perm.order() {
        match fixed[p] { Some(id) => { prefix[n] = id; n += 1 } None => break }
    }
    (perm, prefix, n)
}

fn checked_count(a:u64,b:u64)->u64 {
    a.checked_add(b).unwrap_or_else(|| storage_failure(std::io::Error::new(std::io::ErrorKind::InvalidData,"aggregate count overflow")))
}

fn checked_count_product(a: u64, b: u64) -> u64 {
    a.checked_mul(b).unwrap_or_else(|| {
        storage_failure(std::io::Error::new(std::io::ErrorKind::InvalidData, "aggregate count overflow"))
    })
}

/// Leapfrog plan shared by `leapfrog` and `count_leapfrog`: the global variable order (the optimizer's `given` order
/// when it is a permutation of the BGP's variables, otherwise greedy: connected first, then smallest constant-prefix
/// count) and, per pattern, the permutation keyed (constants, then variables in that order) with its reordered slots.
fn lf_plan(pats: &[[Slot; 3]], counts: &[u64], given: Option<Vec<usize>>) -> (Vec<usize>, Vec<(Perm, [Slot; 3])>) {
    let all: HashSet<usize> = pats.iter().flat_map(vars_of).collect();
    let mut order: Vec<usize> = given.filter(|o| o.len() == all.len() && o.iter().copied().collect::<HashSet<_>>().len() == all.len() && o.iter().all(|v| all.contains(v))).unwrap_or_default();
    while order.len() < all.len() {
        let cand = all.iter().filter(|v| !order.contains(v)).min_by_key(|&&v| {
            let connected = order.is_empty() || pats.iter().any(|p| { let vs = vars_of(p); vs.contains(&v) && vs.iter().any(|x| order.contains(x)) });
            let best = pats.iter().zip(counts).filter(|(p, _)| vars_of(p).contains(&v)).map(|(_, c)| *c).min().unwrap_or(u64::MAX);
            (!connected, best, v)
        }).copied().unwrap();
        order.push(cand);
    }
    let rank: HashMap<usize, usize> = order.iter().enumerate().map(|(i, &v)| (v, i)).collect();
    let plans = pats.iter().map(|p| {
        let mut pos = [0usize, 1, 2];
        pos.sort_by_key(|&i| match p[i] { Slot::Const(_) => (0, 0), Slot::Var(v) => (1, rank[&v]) });
        let perm = *Perm::ALL.iter().find(|q| q.order() == pos).unwrap();
        (perm, [p[pos[0]], p[pos[1]], p[pos[2]]])
    }).collect();
    (order, plans)
}

#[cfg(test)]
mod extension_bench {
    use super::*;
    use std::time::Instant;

    /// The previous `extend_seq` row loop for the `matching_rows` layout, kept here only as the microbenchmark baseline.
    fn old_rows(out: &mut Vec<TermId>, row: &[TermId], pat: &[Slot; 3], order: &[usize; 3], n: usize, rows: &[[TermId; 3]]) {
        'mrows: for t in rows {
            let start = out.len();
            out.extend_from_slice(row);
            for (k, &pos) in order.iter().enumerate().skip(n) {
                if let Slot::Var(v) = pat[pos] {
                    let val = t[k];
                    let cell = &mut out[start + v];
                    if *cell == UNBOUND { *cell = val } else if *cell != val { out.truncate(start); continue 'mrows }
                }
            }
        }
    }
    fn new_rows(out: &mut Vec<TermId>, row: &[TermId], pat: &[Slot; 3], order: &[usize; 3], n: usize, rows: &[[TermId; 3]]) {
        for t in rows { push_extension(out, row, pat, order, n, |k| t[k]); }
    }
    fn input() -> (Vec<[TermId; 3]>, Vec<Vec<TermId>>) {
        // Index rows sorted on the first column; every 7th row shares its object with the pre-bound variable so the
        // conflict (truncate) path runs too.
        let rows: Vec<[TermId; 3]> = (0..5000u64).map(|i| [TermId(1 + i / 5), TermId(10 + i % 13), TermId(100 + i % 7)]).collect();
        let outer: Vec<Vec<TermId>> = (0..200u64).map(|j| vec![TermId(1 + j), UNBOUND, if j % 2 == 0 { TermId(100 + j % 7) } else { UNBOUND }]).collect();
        (rows, outer)
    }

    /// `cargo test --release -p triplelight-exec extension_bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn push_extension_is_not_slower_than_the_inlined_loop() {
        let (rows, outer) = input();
        let pat = [Slot::Var(0), Slot::Var(1), Slot::Var(2)];
        let order = [0, 1, 2];
        let n = 1;
        // Same output first.
        let (mut a, mut b) = (Vec::new(), Vec::new());
        for row in &outer { old_rows(&mut a, row, &pat, &order, n, &rows); new_rows(&mut b, row, &pat, &order, n, &rows); }
        assert_eq!(a, b, "the helper must produce identical rows");
        let per_round = (outer.len() * rows.len()) as f64;
        let (mut old_ns, mut new_ns) = (Vec::new(), Vec::new());
        for round in 0..9 {
            // Alternate which variant goes first so order effects cancel.
            for variant in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                let mut out = Vec::with_capacity(a.len());
                let t = Instant::now();
                for row in &outer {
                    if variant == 0 { old_rows(&mut out, row, &pat, &order, n, &rows) } else { new_rows(&mut out, row, &pat, &order, n, &rows) }
                }
                let ns = t.elapsed().as_nanos() as f64 / per_round;
                std::hint::black_box(&out);
                if variant == 0 { old_ns.push(ns) } else { new_ns.push(ns) }
            }
        }
        old_ns.sort_by(f64::total_cmp);
        new_ns.sort_by(f64::total_cmp);
        let (o, nw) = (old_ns[old_ns.len() / 2], new_ns[new_ns.len() / 2]);
        eprintln!("extend loop, ns per index row (median of 9, interleaved): old {o:.2}, new {nw:.2}, ratio {:.3}", nw / o);
        assert!(nw <= o * 1.05, "new is more than 5% slower");
    }
}

#[cfg(test)]
pub(crate) mod sorted_probe_tests {
    use super::*;
    use crate::storage::{Dictionary, StorageError};
    use triplelight_rdf::{Term, TermRef};

    /// Triples in every key order, scanned from sorted arrays: just enough of a store for `extend` (and for the
    /// path tests in `path.rs`).
    pub(crate) struct Triples { by_perm: Vec<(Perm, Vec<[TermId; 3]>)> }
    impl Triples {
        pub(crate) fn new(t: &[[u64; 3]]) -> Self {
            let by_perm = Perm::ALL.into_iter().map(|perm| {
                let o = perm.order();
                let mut rows: Vec<[TermId; 3]> = t.iter().map(|x| o.map(|i| TermId(x[i]))).collect();
                rows.sort();
                rows.dedup();
                (perm, rows)
            }).collect();
            Self { by_perm }
        }
        fn matching(&self, perm: Perm, prefix: &[TermId]) -> Vec<[TermId; 3]> {
            let rows = &self.by_perm.iter().find(|(p, _)| *p == perm).unwrap().1;
            let k = prefix.len();
            let lo = rows.partition_point(|r| &r[..k] < prefix);
            let hi = rows.partition_point(|r| &r[..k] <= prefix);
            rows[lo..hi].to_vec()
        }
    }
    impl Dictionary for Triples {
        fn lookup(&self, _: &Term) -> Result<Option<TermId>, StorageError> { Ok(None) }
        fn resolve(&self, _: TermId) -> Result<Term, StorageError> { Err(StorageError::other("encoded-only fixture")) }
        fn with_term(&self, _: TermId, _: &mut dyn FnMut(TermRef<'_>)) -> Result<(), StorageError> { Err(StorageError::other("encoded-only fixture")) }
    }
    impl Snapshot for Triples {
        fn scan<'b>(&'b self, perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'b> {
            let mut b = TripleBatch::default();
            for r in self.matching(perm, prefix) {
                for (i, id) in r.into_iter().enumerate() { b.cols[i].push(id) }
            }
            Box::new(std::iter::once(Ok(b)))
        }
        fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> { Ok(self.matching(perm, prefix).len() as u64) }
        fn seek(&self, perm: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> {
            Ok(self.matching(perm, prefix).into_iter().map(|r| r[prefix.len()]).find(|id| *id >= target))
        }
    }

    #[test]
    fn sorted_probe_output_is_identical_and_in_input_order() {
        let mut rng = 0x2545_f491_4f6c_dd1du64;
        let mut next = |m: u64| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng % m
        };
        // Subjects 1..=300 with predicates 1000..=1002; objects 1..=400, so many objects are subjects too.
        let mut t = Vec::new();
        for s in 1..=300u64 {
            for _ in 0..1 + next(4) { t.push([s, 1000 + next(3), 1 + next(400)]) }
        }
        let snap = Triples::new(&t);
        let w = 3;
        let n = SORTED_PROBE_MIN_ROWS + 3000;
        // ?0 bound (duplicates; some keys without matches), ?1 unbound, ?2 sometimes bound (a repeated-variable
        // check against the index value), sometimes UNBOUND.
        let mut rows = Vec::with_capacity(n * w);
        for _ in 0..n {
            rows.push(TermId(1 + next(350)));
            rows.push(UNBOUND);
            rows.push(if next(3) == 0 { TermId(1 + next(400)) } else { UNBOUND });
        }
        rows[2] = UNBOUND; // the first row decides the key: only ?0 bound
        rows[w] = rows[0]; // a duplicate key next to it
        let on = Bgp { snap: &snap, w, ranges: None, sorted_probe: true, interrupt: None, budget: None };
        let off = Bgp { snap: &snap, w, ranges: None, sorted_probe: false, interrupt: None, budget: None };
        let patterns = [
            [Slot::Var(0), Slot::Const(Some(TermId(1000))), Slot::Var(1)], // key as subject
            [Slot::Var(1), Slot::Const(Some(TermId(1001))), Slot::Var(0)], // key as object
            [Slot::Var(0), Slot::Var(1), Slot::Var(2)],                    // ?2 bound in some rows only
            [Slot::Var(0), Slot::Const(Some(TermId(1002))), Slot::Var(0)], // repeated key variable
        ];
        for pat in &patterns {
            let sorted = on.extend_sorted(pat, &rows).expect("unsorted input with a single bound key");
            assert_eq!(sorted, off.extend_seq(pat, &rows), "{pat:?}");
            assert_eq!(on.extend(pat, &rows), off.extend(pat, &rows), "{pat:?}");
        }
        // Not taken: an UNBOUND key in some row, two bound key variables, input already in key order, or off.
        let mut holes = rows.clone();
        holes[w * 5] = UNBOUND;
        assert!(on.extend_sorted(&patterns[0], &holes).is_none());
        assert_eq!(on.extend(&patterns[0], &holes), off.extend(&patterns[0], &holes));
        let mut both = rows.clone();
        both[2] = TermId(7); // first row binds ?2 too
        assert!(on.extend_sorted(&patterns[2], &both).is_none());
        let mut in_order: Vec<TermId> = Vec::with_capacity(rows.len());
        let mut keyed: Vec<&[TermId]> = rows.chunks_exact(w).collect();
        keyed.sort_by_key(|r| r[0]);
        for r in keyed { in_order.extend_from_slice(r) }
        assert!(on.extend_sorted(&patterns[0], &in_order).is_none());
        assert!(off.extend_sorted(&patterns[0], &rows).is_none());
        // Below the threshold.
        assert!(on.extend_sorted(&patterns[0], &rows[..w * (SORTED_PROBE_MIN_ROWS - 1)]).is_none());
    }

    /// The merge join over key-sorted rows equals the probe join row for row: duplicate keys, keys without matches,
    /// keys past the last index key, morsel boundaries inside a key's run, the key as subject or as object.
    #[test]
    fn merge_join_matches_probes_in_order() {
        let mut t = Vec::new();
        for s in 1..=400u64 {
            for j in 0..(s % 5) { t.push([s, 1000, 1 + (s * 7 + j * 13) % 500]) }
            if s % 3 == 0 { t.push([1 + (s * 11) % 450, 1001, s]) }
        }
        let snap = Triples::new(&t);
        let w = 3;
        let mut keys: Vec<u64> = (0..MERGE_MORSEL as u64 * 2 + 777).map(|i| 1 + (i * 2654435761) % 470).collect();
        keys.sort_unstable();
        let rows: Vec<TermId> = keys.iter().flat_map(|&k| [TermId(k), UNBOUND, TermId(9)]).collect();
        let b = Bgp { snap: &snap, w, ranges: None, sorted_probe: false, interrupt: None, budget: None };
        for pat in [
            [Slot::Var(0), Slot::Const(Some(TermId(1000))), Slot::Var(1)],
            [Slot::Var(1), Slot::Const(Some(TermId(1001))), Slot::Var(0)],
        ] {
            let merged = b.extend_merge(&pat, &rows).expect("sorted keys, dense enough");
            assert_eq!(merged, b.extend_seq(&pat, &rows), "{pat:?}");
        }
        // Not taken: unsorted keys, or the free variable already bound.
        let mut unsorted = rows.clone();
        unsorted.swap(0, w * 100);
        assert!(b.extend_merge(&[Slot::Var(0), Slot::Const(Some(TermId(1000))), Slot::Var(1)], &unsorted).is_none());
        assert!(b.extend_merge(&[Slot::Var(0), Slot::Const(Some(TermId(1000))), Slot::Var(2)], &rows).is_none());
    }

    /// The hash step charges its tables and output (#111): with no budget nothing changes, a step that fits a
    /// generous budget gives the same rows and returns every byte it held, and one whose tables cannot fit ends the
    /// query with `ResourceLimit` before building them.
    #[test]
    fn the_hash_step_fits_a_budget_it_fits_and_refuses_one_it_does_not() {
        let t: Vec<[u64; 3]> = (1..=3000u64).map(|s| [s, 1000, 5000 + s % 97]).collect();
        let snap = Triples::new(&t);
        let w = 2;
        // Unsorted keys (no merge join), every key bound and the object free: the whole-range hash table.
        let rows: Vec<TermId> = (0..2000u64).flat_map(|i| [TermId(1 + (i * 2654435761) % 3000), UNBOUND]).collect();
        let pat = [Slot::Var(0), Slot::Const(Some(TermId(1000))), Slot::Var(1)];
        let free = Bgp { snap: &snap, w, ranges: None, sorted_probe: false, interrupt: None, budget: None };
        let want = free.extend_hash(&pat, &rows).expect("the hash path applies");
        assert_eq!(want, free.extend_seq(&pat, &rows));
        let roomy = super::super::MemoryBudget::new(64 << 20);
        let b = Bgp { budget: Some(&roomy), ..free };
        assert_eq!(b.extend_hash(&pat, &rows).unwrap(), want);
        assert_eq!(roomy.used(), 0, "the step's charge is returned");
        let tight = super::super::MemoryBudget::new(4 << 10);
        let b = Bgp { budget: Some(&tight), ..free };
        let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.extend_hash(&pat, &rows)));
        assert!(stopped.is_err_and(|p| p.is::<super::super::limits::OverBudgetStop>()));
        assert_eq!(tight.used(), 0);
    }

    /// A table is charged at its allocated bucket count: 3,584 entries fit 4,096 buckets at 7/8, 3,585 need 8,192.
    #[test]
    fn hash_tables_are_charged_at_their_bucket_capacity() {
        assert_eq!(table_bytes(3584, HASH_MAP_SLOT), 4096 * 17 + 16);
        assert_eq!(table_bytes(3585, HASH_MAP_SLOT), 8192 * 17 + 16);
        assert_eq!(table_bytes(0, HASH_SET_SLOT), 4 * 9 + 16);
    }

    /// The whole-range map holds one entry per distinct scanned key, which can exceed the input's rows: a budget
    /// that would fit a map sized by the input refuses the step (#366 review).
    #[test]
    fn the_whole_range_map_is_charged_for_every_distinct_key() {
        let t: Vec<[u64; 3]> = (1..=3000u64).map(|s| [s, 1000, 5000 + s % 97]).collect();
        let snap = Triples::new(&t);
        let rows: Vec<TermId> = (0..1500u64).flat_map(|i| [TermId(1 + (i * 2654435761) % 3000), UNBOUND]).collect();
        let pat = [Slot::Var(0), Slot::Const(Some(TermId(1000))), Slot::Var(1)];
        // A map sized by the 1,500 input rows needs table_bytes(1500) + 3,000 x 8 bytes; 3,000 keys need a larger table.
        let by_rows = table_bytes(1500, HASH_MAP_SLOT) + 3000 * 8;
        assert!(table_bytes(3000, HASH_MAP_SLOT) > table_bytes(1500, HASH_MAP_SLOT), "the test needs a larger table");
        let budget = super::super::MemoryBudget::new(by_rows + 1024);
        let b = Bgp { snap: &snap, w: 2, ranges: None, sorted_probe: false, interrupt: None, budget: Some(&budget) };
        let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.extend_hash(&pat, &rows)));
        assert!(stopped.is_err_and(|p| p.is::<super::super::limits::OverBudgetStop>()));
        assert_eq!(budget.used(), 0);
    }

    /// A semi-join keeps every value of a kept key, so its values can outgrow the input's row count: they are charged
    /// as they grow, and a step whose values outgrow the budget stops before the reallocation (#366 review).
    #[test]
    fn semi_join_values_are_charged_as_they_grow() {
        // 50 subjects x 100 values each; 1,024 input rows over keys 1..=20, so 2,000 values are kept for 1,024 rows.
        let t: Vec<[u64; 3]> = (1..=50u64).flat_map(|s| (0..100u64).map(move |j| [s, 1000, 10_000 + s * 1000 + j])).collect();
        let snap = Triples::new(&t);
        let rows: Vec<TermId> = (0..1024u64).flat_map(|i| [TermId(1 + i % 20), UNBOUND]).collect();
        let pat = [Slot::Var(0), Slot::Const(Some(TermId(1000))), Slot::Var(1)];
        let ranges = Ranges::default();
        let free = Bgp { snap: &snap, w: 2, ranges: Some(&ranges), sorted_probe: false, interrupt: None, budget: None };
        let want = free.extend_hash(&pat, &rows).expect("the semi-join applies");
        assert_eq!(want, free.extend_seq(&pat, &rows));
        let roomy = super::super::MemoryBudget::new(64 << 20);
        assert_eq!(Bgp { budget: Some(&roomy), ..free }.extend_hash(&pat, &rows).unwrap(), want);
        assert_eq!(roomy.used(), 0);
        // Enough for the up-front charge (values sized by the 1,024 rows), not for the values' growth past them.
        let up_front = table_bytes(1024, HASH_SET_SLOT) + table_bytes(1024, HASH_MAP_SLOT) + 1024 * 8;
        let tight = super::super::MemoryBudget::new(up_front + 512);
        let b = Bgp { budget: Some(&tight), ..free };
        let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.extend_hash(&pat, &rows)));
        assert!(stopped.is_err_and(|p| p.is::<super::super::limits::OverBudgetStop>()));
        assert_eq!(tight.used(), 0);
    }
}
