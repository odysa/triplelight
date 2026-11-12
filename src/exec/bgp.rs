//! Basic-graph-pattern evaluation over flat row buffers (`w` IDs per row, no per-row allocation), parallelized
//! morsel-style: large inputs are split into chunks that rayon workers extend independently.
use super::{storage_failure, UNBOUND};
use crate::model::{Perm, TermId};
use crate::storage::Snapshot;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};

/// Rows per parallel work unit.
const MORSEL: usize = 1024;

#[derive(Copy, Clone, Debug)]
pub(super) enum Slot { Const(Option<TermId>), Var(usize) }

pub(super) struct Bgp<'a, S: Snapshot + ?Sized> {
    pub snap: &'a S,
    /// Row width (number of query variables).
    pub w: usize,
}

impl<'a, S: Snapshot + ?Sized> Bgp<'a, S> {
    fn count(&self, perm: Perm, prefix: &[TermId]) -> u64 { self.snap.count(perm, prefix).unwrap_or_else(|e| storage_failure(e)) }
    fn seek(&self, perm: Perm, prefix: &[TermId], t: TermId) -> Option<TermId> {
        self.snap.seek(perm, prefix, t).unwrap_or_else(|e| storage_failure(e))
    }

    /// Greedy order with exact index counts, evaluated by index nested-loop joins.
    pub fn eval_greedy(&self, pats: &[[Slot; 3]], mut rows: Vec<TermId>) -> Vec<TermId> {
        let w = self.w;
        if rows.is_empty() { return rows }
        let mut remaining: Vec<usize> = (0..pats.len()).collect();
        // Variables bound in every input row.
        let mut bound: HashSet<usize> = (0..w).filter(|&i| rows.chunks(w).all(|r| r[i] != UNBOUND)).collect();
        while !remaining.is_empty() && !rows.is_empty() {
            let sample = rows[..w].to_vec();
            let (k, _) = remaining.iter().enumerate().map(|(k, &pi)| (k, self.estimate(&pats[pi], &bound, &sample)))
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).unwrap();
            let pi = remaining.remove(k);
            rows = self.extend(&pats[pi], &rows);
            for s in &pats[pi] { if let Slot::Var(v) = s { bound.insert(*v); } }
        }
        rows
    }

    pub fn eval_order(&self, pats: &[[Slot; 3]], order: &[usize], mut rows: Vec<TermId>) -> Vec<TermId> {
        for &pi in order { if rows.is_empty() { break } rows = self.extend(&pats[pi], &rows); }
        rows
    }

    fn estimate(&self, pat: &[Slot; 3], bound: &HashSet<usize>, sample: &[TermId]) -> f64 {
        let fixed: [Option<TermId>; 3] = std::array::from_fn(|i| match pat[i] {
            Slot::Const(c) => c,
            Slot::Var(v) if bound.contains(&v) => Some(sample[v]),
            _ => None,
        });
        let (perm, prefix, n) = choose_perm(&fixed);
        let c = self.count(perm, &prefix[..n]) as f64;
        let connected = pat.iter().any(|s| matches!(s, Slot::Var(v) if bound.contains(v)));
        if bound.is_empty() || connected { c } else { c * 1e6 } // avoid cross products
    }

    /// Index nested-loop join of `rows` with one pattern: one index range read per input row.
    pub fn extend(&self, pat: &[Slot; 3], rows: &[TermId]) -> Vec<TermId> {
        let w = self.w;
        if rows.len() / w.max(1) >= 2 * MORSEL {
            let parts: Vec<Vec<TermId>> = rows.par_chunks(w * MORSEL).map(|c| self.extend_seq(pat, c)).collect();
            parts.concat()
        } else {
            self.extend_seq(pat, rows)
        }
    }

    fn extend_seq(&self, pat: &[Slot; 3], rows: &[TermId]) -> Vec<TermId> {
        let w = self.w;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows.chunks_exact(w.max(1)) {
            let row = &row[..w];
            let fixed: [Option<TermId>; 3] = std::array::from_fn(|i| match pat[i] {
                Slot::Const(c) => c,
                Slot::Var(v) => (row[v] != UNBOUND).then_some(row[v]),
            });
            let (perm, prefix, n) = choose_perm(&fixed);
            if n == 3 {
                // Fully bound: a membership probe, no scan.
                if self.count(perm, &prefix) > 0 { out.extend_from_slice(row) }
                continue;
            }
            let order = perm.order();
            // Which key columns bind new variables, and which must equal an already-bound value (repeated variable).
            for batch in self.snap.scan(perm, &prefix[..n]) {
                let batch = batch.unwrap_or_else(|e| storage_failure(e));
                'rows: for i in 0..batch.len() {
                    let start = out.len();
                    out.extend_from_slice(row);
                    for (k, &pos) in order.iter().enumerate().skip(n) {
                        if let Slot::Var(v) = pat[pos] {
                            let val = batch.cols[k][i];
                            let cell = &mut out[start + v];
                            if *cell == UNBOUND { *cell = val } else if *cell != val { out.truncate(start); continue 'rows }
                        }
                    }
                }
            }
        }
        out
    }

    /// Leapfrog triejoin (Veldhuizen, ICDT'14), the worst-case-optimal join: bind one variable at a time,
    /// intersecting by index seeks the sorted value lists of every pattern containing it. Each pattern reads the
    /// permutation keyed (its constants, then its variables in the global order). The first variable's values are
    /// split across rayon workers.
    pub fn leapfrog(&self, pats: &[[Slot; 3]], row: &[TermId], given: Option<Vec<usize>>) -> Vec<TermId> {
        let counts: Vec<u64> = pats.iter().map(|p| {
            let fixed: [Option<TermId>; 3] = std::array::from_fn(|i| if let Slot::Const(c) = p[i] { c } else { None });
            let (perm, prefix, n) = choose_perm(&fixed);
            self.count(perm, &prefix[..n])
        }).collect();
        if counts.contains(&0) { return Vec::new() }
        let vars_of = |p: &[Slot; 3]| -> Vec<usize> { p.iter().filter_map(|s| if let Slot::Var(v) = s { Some(*v) } else { None }).collect() };
        let all: HashSet<usize> = pats.iter().flat_map(vars_of).collect();
        let mut order: Vec<usize> = given.filter(|o| o.len() == all.len() && o.iter().all(|v| all.contains(v))).unwrap_or_default();
        while order.len() < all.len() {
            let cand = all.iter().filter(|v| !order.contains(v)).min_by_key(|&&v| {
                let connected = order.is_empty() || pats.iter().any(|p| { let vs = vars_of(p); vs.contains(&v) && vs.iter().any(|x| order.contains(x)) });
                let best = pats.iter().zip(&counts).filter(|(p, _)| vars_of(p).contains(&v)).map(|(_, c)| *c).min().unwrap_or(u64::MAX);
                (!connected, best, v)
            }).copied().unwrap();
            order.push(cand);
        }
        let rank: HashMap<usize, usize> = order.iter().enumerate().map(|(i, &v)| (v, i)).collect();
        let plans: Vec<(Perm, [Slot; 3])> = pats.iter().map(|p| {
            let mut pos = [0usize, 1, 2];
            pos.sort_by_key(|&i| match p[i] { Slot::Const(_) => (0, 0), Slot::Var(v) => (1, rank[&v]) });
            let perm = *Perm::ALL.iter().find(|q| q.order() == pos).unwrap();
            (perm, [p[pos[0]], p[pos[1]], p[pos[2]]])
        }).collect();
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

    /// All values of `v` on which every pattern containing it agrees, given the bound prefix in `row`.
    fn intersect(&self, plans: &[(Perm, [Slot; 3])], v: usize, row: &[TermId]) -> Vec<TermId> {
        let mut out = Vec::new();
        self.leap(plans, v, row, |x| out.push(x));
        out
    }

    fn leap(&self, plans: &[(Perm, [Slot; 3])], v: usize, row: &[TermId], mut emit: impl FnMut(TermId)) {
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
                let Some(y) = self.seek(*perm, &prefix[..*k], x) else { break 'outer };
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

/// GYO reduction: a BGP is acyclic iff repeatedly removing variables that occur in one pattern only, and patterns
/// whose variables are contained in another pattern's, empties the hypergraph.
pub(super) fn cyclic(pats: &[[Slot; 3]]) -> bool {
    let mut edges: Vec<HashSet<usize>> = pats.iter()
        .map(|p| p.iter().filter_map(|s| if let Slot::Var(v) = s { Some(*v) } else { None }).collect()).collect();
    loop {
        let mut changed = false;
        let mut occ: HashMap<usize, usize> = HashMap::new();
        for e in &edges { for v in e { *occ.entry(*v).or_default() += 1 } }
        for e in edges.iter_mut() {
            let before = e.len();
            e.retain(|v| occ[v] > 1);
            changed |= e.len() != before;
        }
        let n = edges.len();
        let mut keep = vec![true; n];
        for i in 0..n {
            if edges[i].is_empty() { keep[i] = false; continue }
            if (0..n).any(|j| j != i && keep[j] && edges[i].is_subset(&edges[j]) && (edges[i] != edges[j] || j < i)) { keep[i] = false }
        }
        let mut k = keep.iter();
        edges.retain(|_| *k.next().unwrap());
        changed |= edges.len() != n;
        if edges.is_empty() { return false }
        if !changed { return true }
    }
}

pub(super) fn no_repeated_var(p: &[Slot; 3]) -> bool {
    let vs: Vec<usize> = p.iter().filter_map(|s| if let Slot::Var(v) = s { Some(*v) } else { None }).collect();
    vs.iter().collect::<HashSet<_>>().len() == vs.len()
}
