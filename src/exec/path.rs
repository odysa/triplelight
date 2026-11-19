//! SPARQL 1.1 property paths (§9, §18.4) evaluated on term IDs.
//!
//! Multiplicity follows the spec: a fixed-length path (a link, `^`, `/`, `|`, `!`) is the same as the triple patterns
//! it abbreviates, so duplicates through different intermediate nodes are kept; `*`, `+` and `?` are evaluated as the
//! set of nodes reachable from each start node, so they never repeat a pair. A zero-length path (`*`, `?`) matches a
//! bound endpoint to itself even if that term is not in the graph, and with both endpoints unbound it matches every
//! subject and object of the graph to itself.
use super::{storage_failure, Ctx, Row, UNBOUND};
use crate::model::{Perm, TermId};
use crate::storage::Snapshot;
use crate::sparql::PropertyPathExpression as Ppe;
use crate::sparql::TermPattern;
use std::collections::{HashMap, HashSet};

/// A path with its IRIs resolved to IDs (`None` = not in the store, so that link never matches).
enum P {
    Link(Option<TermId>),
    Rev(Box<P>),
    Seq(Box<P>, Box<P>),
    Alt(Box<P>, Box<P>),
    Star(Box<P>),
    Plus(Box<P>),
    Opt(Box<P>),
    /// `!(p1|…|pn)` in the forward direction; the parser turns inverse members into `^!(…)`.
    Neg(Vec<Option<TermId>>),
}

impl<S: Snapshot + ?Sized> Ctx<'_, S> {
    fn resolve_path(&self, p: &Ppe) -> P {
        match p {
            Ppe::NamedNode(n) => P::Link(self.lookup(&n.clone().into())),
            Ppe::Reverse(q) => P::Rev(Box::new(self.resolve_path(q))),
            Ppe::Sequence(a, b) => P::Seq(Box::new(self.resolve_path(a)), Box::new(self.resolve_path(b))),
            Ppe::Alternative(a, b) => P::Alt(Box::new(self.resolve_path(a)), Box::new(self.resolve_path(b))),
            Ppe::ZeroOrMore(q) => P::Star(Box::new(self.resolve_path(q))),
            Ppe::OneOrMore(q) => P::Plus(Box::new(self.resolve_path(q))),
            Ppe::ZeroOrOne(q) => P::Opt(Box::new(self.resolve_path(q))),
            Ppe::NegatedPropertySet(ns) => P::Neg(ns.iter().map(|n| self.lookup(&n.clone().into())).collect()),
        }
    }

    /// Calls `f` for every triple matching `prefix` in `perm` (columns in that permutation's key order).
    pub(super) fn scan_each(&self, perm: Perm, prefix: &[TermId], mut f: impl FnMut(TermId, TermId, TermId)) {
        for batch in self.snap.scan(perm, prefix) {
            let b = batch.unwrap_or_else(|e| storage_failure(e));
            for i in 0..b.len() { f(b.cols[0][i], b.cols[1][i], b.cols[2][i]) }
        }
    }

    /// Nodes reached from `x` by one traversal of `p` (`rev`: traverse it backwards), as a multiset.
    fn step(&self, p: &P, rev: bool, x: TermId, out: &mut Vec<TermId>) {
        match p {
            P::Link(None) => {}
            P::Link(Some(id)) => {
                if rev { self.scan_each(Perm::Pos, &[*id, x], |_, _, s| out.push(s)) }
                else { self.scan_each(Perm::Spo, &[x, *id], |_, _, o| out.push(o)) }
            }
            P::Rev(q) => self.step(q, !rev, x, out),
            P::Seq(a, b) => {
                let (first, second) = if rev { (b, a) } else { (a, b) };
                let mut mid = Vec::new();
                self.step(first, rev, x, &mut mid);
                for y in mid { self.step(second, rev, y, out) }
            }
            P::Alt(a, b) => { self.step(a, rev, x, out); self.step(b, rev, x, out) }
            P::Neg(ns) => {
                let excluded: HashSet<TermId> = ns.iter().flatten().copied().collect();
                if rev { self.scan_each(Perm::Osp, &[x], |_, s, pr| if !excluded.contains(&pr) { out.push(s) }) }
                else { self.scan_each(Perm::Spo, &[x], |_, pr, o| if !excluded.contains(&pr) { out.push(o) }) }
            }
            P::Star(q) | P::Plus(q) | P::Opt(q) => out.extend(self.closure(p, q, rev, x)),
        }
    }

    /// Distinct nodes reachable from `x` by `*` / `+` / `?` over `q`, in discovery order.
    fn closure(&self, p: &P, q: &P, rev: bool, x: TermId) -> Vec<TermId> {
        let zero = matches!(p, P::Star(_) | P::Opt(_));
        let mut seen = HashSet::new();
        let mut order = Vec::new();
        if zero { seen.insert(x); order.push(x); }
        let mut next = Vec::new();
        self.step(q, rev, x, &mut next);
        if matches!(p, P::Opt(_)) {
            for y in next { if seen.insert(y) { order.push(y) } }
            return order;
        }
        let mut frontier = next;
        while !frontier.is_empty() {
            let mut grown = Vec::new();
            for y in frontier {
                if seen.insert(y) {
                    order.push(y);
                    self.step(q, rev, y, &mut grown);
                }
            }
            frontier = grown;
        }
        order
    }

    /// Every (start, end) pair of `p` when neither endpoint is bound, with the same multiplicities as `step`.
    fn all_pairs(&self, p: &P, out: &mut Vec<(TermId, TermId)>) {
        match p {
            P::Link(None) => {}
            P::Link(Some(id)) => self.scan_each(Perm::Pso, &[*id], |_, s, o| out.push((s, o))),
            P::Rev(q) => { let mut v = Vec::new(); self.all_pairs(q, &mut v); out.extend(v.into_iter().map(|(a, b)| (b, a))) }
            P::Seq(a, b) => {
                let mut first = Vec::new();
                self.all_pairs(a, &mut first);
                let mut memo: HashMap<TermId, Vec<TermId>> = HashMap::new();
                for (x, y) in first {
                    let ends = memo.entry(y).or_insert_with(|| { let mut v = Vec::new(); self.step(b, false, y, &mut v); v });
                    out.extend(ends.iter().map(|&z| (x, z)));
                }
            }
            P::Alt(a, b) => { self.all_pairs(a, out); self.all_pairs(b, out) }
            P::Neg(ns) => {
                let excluded: HashSet<TermId> = ns.iter().flatten().copied().collect();
                self.scan_each(Perm::Spo, &[], |s, pr, o| if !excluded.contains(&pr) { out.push((s, o)) })
            }
            P::Plus(q) => {
                // Start nodes are those with at least one outgoing step.
                let mut firsts = Vec::new();
                self.all_pairs(q, &mut firsts);
                let mut starts: Vec<TermId> = firsts.into_iter().map(|(s, _)| s).collect();
                dedup_keep_order(&mut starts);
                for x in starts { out.extend(self.closure(p, q, false, x).into_iter().map(|z| (x, z))) }
            }
            P::Star(q) | P::Opt(q) => {
                for x in self.graph_nodes() { out.extend(self.closure(p, q, false, x).into_iter().map(|z| (x, z))) }
            }
        }
    }

    fn is_graph_node(&self, x: TermId) -> bool {
        let n = |perm, x| self.snap.count(perm, &[x]).unwrap_or_else(|e| storage_failure(e));
        n(Perm::Spo, x) > 0 || n(Perm::Osp, x) > 0
    }

    /// Every subject and object of the graph, once each (the domain of a zero-length path with no bound endpoint).
    fn graph_nodes(&self) -> Vec<TermId> {
        let mut nodes = Vec::new();
        let mut last = UNBOUND;
        self.scan_each(Perm::Spo, &[], |s, _, _| if s != last { nodes.push(s); last = s; });
        last = UNBOUND;
        self.scan_each(Perm::Osp, &[], |o, _, _| if o != last { nodes.push(o); last = o; });
        dedup_keep_order(&mut nodes);
        nodes
    }

    /// The ID an endpoint has in `row`: a constant (interned, so a term absent from the store still matches itself
    /// on a zero-length path), a bound variable, or `None` if it is an unbound variable.
    fn endpoint(&self, tp: &TermPattern, row: &Row) -> Result<Option<TermId>, usize> {
        match tp {
            TermPattern::Variable(v) => { let i = self.var(v); if row[i] == UNBOUND { Err(i) } else { Ok(Some(row[i])) } }
            TermPattern::BlankNode(b) => { let i = self.vars[&format!("_b{}", b.as_str())]; if row[i] == UNBOUND { Err(i) } else { Ok(Some(row[i])) } }
            TermPattern::NamedNode(n) => Ok(Some(self.intern(&n.clone().into()))),
            TermPattern::Literal(l) => Ok(Some(self.intern(&l.clone().into()))),
            #[allow(unreachable_patterns)]
            _ => Ok(None),
        }
    }

    pub(super) fn eval_path(&self, subject: &TermPattern, path: &Ppe, object: &TermPattern, input: Vec<Row>) -> Vec<Row> {
        let p = self.resolve_path(path);
        let mut out = Vec::new();
        // Rows that bind the endpoints the same way share one traversal.
        let mut memo: HashMap<(Option<TermId>, Option<TermId>), Vec<(TermId, TermId)>> = HashMap::new();
        for row in input {
            let s = self.endpoint(subject, &row);
            let o = self.endpoint(object, &row);
            // A variable endpoint bound by an earlier pattern (VALUES, BIND, a join) stands for a solution of the path
            // evaluated on its own, whose zero-length matches range only over the graph's nodes. A term that occurs
            // in no triple can match no step either, so such a row has no solutions. (A constant written in the path
            // itself is different: `<x> :p* ?y` matches <x> even if <x> is not in the graph.)
            if [(subject, s), (object, o)].iter().any(|(tp, e)| matches!(tp, TermPattern::Variable(_) | TermPattern::BlankNode(_))
                && matches!(e, Ok(Some(x)) if !self.is_graph_node(*x))) { continue }
            let key = (s.ok().flatten(), o.ok().flatten());
            let pairs = memo.entry(key).or_insert_with(|| match key {
                (Some(x), Some(y)) => { let mut v = Vec::new(); self.step(&p, false, x, &mut v); v.into_iter().filter(|&z| z == y).map(|z| (x, z)).collect() }
                (Some(x), None) => { let mut v = Vec::new(); self.step(&p, false, x, &mut v); v.into_iter().map(|z| (x, z)).collect() }
                (None, Some(y)) => { let mut v = Vec::new(); self.step(&p, true, y, &mut v); v.into_iter().map(|z| (z, y)).collect() }
                (None, None) => { let mut v = Vec::new(); self.all_pairs(&p, &mut v); v }
            });
            for &(a, b) in pairs.iter() {
                let mut r = row.clone();
                if let Err(i) = s { r[i] = a }
                if let Err(j) = o {
                    // `?x path ?x`: the second occurrence must agree with the first.
                    if r[j] != UNBOUND && r[j] != b { continue }
                    r[j] = b
                }
                out.push(r);
            }
        }
        out
    }
}

fn dedup_keep_order(v: &mut Vec<TermId>) {
    let mut seen = HashSet::new();
    v.retain(|x| seen.insert(*x));
}
