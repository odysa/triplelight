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
    fn count(&self, perm: Perm, prefix: &[TermId]) -> u64 { self.snap.count(perm, prefix).unwrap_or_else(|e| storage_failure(e)) }
    fn seek(&self, perm: Perm, prefix: &[TermId], t: TermId) -> Option<TermId> {
        self.snap.seek(perm, prefix, t).unwrap_or_else(|e| storage_failure(e))
    }

    /// Greedy order with exact index counts, evaluated by index nested-loop joins.
    /// `post` runs after every join step with the set of variables bound so far (used to apply filters as early as
    /// their variables are bound).
    pub fn eval_greedy(&self, pats: &[[Slot; 3]], mut rows: Vec<TermId>, post: &mut dyn FnMut(&mut Vec<TermId>, &HashSet<usize>)) -> Vec<TermId> {
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
            post(&mut rows, &bound);
        }
        rows
    }

    pub fn eval_order(&self, pats: &[[Slot; 3]], order: &[usize], mut rows: Vec<TermId>, post: &mut dyn FnMut(&mut Vec<TermId>, &HashSet<usize>)) -> Vec<TermId> {
        let w = self.w;
        let mut bound: HashSet<usize> = if rows.is_empty() { HashSet::new() } else { (0..w).filter(|&i| rows.chunks(w).all(|r| r[i] != UNBOUND)).collect() };
        for &pi in order {
            if rows.is_empty() { break }
            rows = self.extend(&pats[pi], &rows);
            for s in &pats[pi] { if let Slot::Var(v) = s { bound.insert(*v); } }
            post(&mut rows, &bound);
        }
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
        if let Some(out) = self.extend_member_set(pat, rows) { return out }
        if let Some(out) = self.extend_hash(pat, rows) { return out }
        if rows.len() / w.max(1) >= 2 * MORSEL {
            let parts: Vec<Vec<TermId>> = rows.par_chunks(w * MORSEL).map(|c| self.extend_seq(pat, c)).collect();
            parts.concat()
        } else {
            self.extend_seq(pat, rows)
        }
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
        let mut set: HashSet<TermId> = HashSet::with_capacity(total as usize);
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
    /// of an index probe. Per row, values come in the same (index) order as the probe join's range scan.
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
        if total > 2 * n_rows as u64 || total > 1 << 24 { return None }
        let mut vals: Vec<TermId> = Vec::with_capacity(total as usize);
        let mut index: HashMap<TermId, (u32, u32)> = HashMap::new();
        for b in self.snap.scan(perm, &[c]) {
            let b = b.unwrap_or_else(|e| storage_failure(e));
            for i in 0..b.len() {
                let (k, f) = (b.cols[1][i], b.cols[2][i]);
                let e = index.entry(k).or_insert((vals.len() as u32, 0));
                e.1 += 1;
                vals.push(f);
            }
        }
        let mut out = Vec::with_capacity(rows.len());
        for r in rows.chunks_exact(w) {
            if let Some(&(start, len)) = index.get(&r[kv]) {
                for &f in &vals[start as usize..(start + len) as usize] {
                    out.extend_from_slice(r);
                    let at = out.len() - w;
                    out[at + fv] = f;
                }
            }
        }
        Some(out)
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
            // Compact layout: rows without their first key column; `t[k - 1]` is key position `k` (k >= n >= 1).
            if let Some(tails) = self.snap.matching_tail(perm, &prefix[..n]) {
                for t in tails { push_extension(&mut out, row, pat, &order, n, |k| t[k - 1]); }
                continue;
            }
            if let Some(rows) = self.snap.matching_rows(perm, &prefix[..n]) {
                for t in rows { push_extension(&mut out, row, pat, &order, n, |k| t[k]); }
                continue;
            }
            // Which key columns bind new variables, and which must equal an already-bound value (repeated variable).
            for batch in self.snap.scan(perm, &prefix[..n]) {
                let batch = batch.unwrap_or_else(|e| storage_failure(e));
                for i in 0..batch.len() { push_extension(&mut out, row, pat, &order, n, |k| batch.cols[k][i]); }
            }
        }
        out
    }

    /// Exact match count of each pattern's constant prefix (its candidate rows before any join).
    fn const_counts(&self, pats: &[[Slot; 3]]) -> Vec<u64> {
        pats.iter().map(|p| {
            let fixed: [Option<TermId>; 3] = std::array::from_fn(|i| if let Slot::Const(c) = p[i] { c } else { None });
            let (perm, prefix, n) = choose_perm(&fixed);
            self.count(perm, &prefix[..n])
        }).collect()
    }

    /// Leapfrog triejoin (Veldhuizen, ICDT'14), the worst-case-optimal join: bind one variable at a time,
    /// intersecting by index seeks the sorted value lists of every pattern containing it. Each pattern reads the
    /// permutation keyed (its constants, then its variables in the global order). The first variable's values are
    /// split across rayon workers.
    pub fn leapfrog(&self, pats: &[[Slot; 3]], row: &[TermId], given: Option<Vec<usize>>) -> Vec<TermId> {
        let counts = self.const_counts(pats);
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
            return pats.iter().map(|p| {
                let fixed=std::array::from_fn(|i|if let Slot::Const(c)=p[i]{c}else{None});
                let (perm,prefix,n)=choose_perm(&fixed);self.count(perm,&prefix[..n])
            }).fold(1u64,|a,b|a.checked_mul(b).unwrap_or_else(||storage_failure(std::io::Error::new(std::io::ErrorKind::InvalidData,"aggregate count overflow"))));
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
        let counts = self.const_counts(pats);
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
            }).fold(1u64, |a,b| a.checked_mul(b).unwrap_or_else(|| storage_failure(std::io::Error::new(std::io::ErrorKind::InvalidData,"aggregate count overflow"))));
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

fn checked_count(a:u64,b:u64)->u64 {
    a.checked_add(b).unwrap_or_else(|| storage_failure(std::io::Error::new(std::io::ErrorKind::InvalidData,"aggregate count overflow")))
}

/// Variables of one pattern, in slot order.
fn vars_of(p: &[Slot; 3]) -> Vec<usize> { p.iter().filter_map(|s| if let Slot::Var(v) = s { Some(*v) } else { None }).collect() }

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
