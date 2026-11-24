//! SPARQL 1.1 property paths (§9, §18.4) evaluated on term IDs.
//!
//! Multiplicity follows the spec: a fixed-length path (a link, `^`, `/`, `|`, `!`) is the same as the triple patterns
//! it abbreviates, so duplicates through different intermediate nodes are kept; `*`, `+` and `?` are evaluated as the
//! set of nodes reachable from each start node, so they never repeat a pair. A zero-length path (`*`, `?`) matches a
//! bound endpoint to itself even if that term is not in the graph, and with both endpoints unbound it matches every
//! subject and object of the graph to itself.
//!
//! # CSR adjacency (`Options::csr_paths`)
//!
//! `p+` / `p*` over a single predicate, possibly reversed (`^p+`, `(^p)*`, `^(p+)`), the common shape
//! (`subOrganizationOf+`, WDBench), can run over a compressed sparse row array of `p`'s edges instead of one index
//! seek per visited node. The array is built from one PSO scan of `p` over the active graph and kept in `Ctx::csr`
//! for the rest of the query, keyed by (predicate, direction, active graph), so a path used in several places (or
//! under many bindings) is built once.
//!
//! * Nodes are the distinct subjects and objects of `p`, sorted by ID and numbered densely by binary search, so
//!   dense order is ID order. `offsets` / `targets` are `u32`. Each direction holds 4 bytes per edge plus 4 bytes
//!   per node of offsets; the node table (8 bytes per node) is shared by both directions. Building peaks at about
//!   20 bytes per edge (the scanned objects, their sorted copy, the mapped targets). Only the direction a closure
//!   needs is built; the other is the transpose of a cached one, without another scan, if one is cached.
//! * Caps: `CsrLimits::max_edges` (50M edges of `p`) and a per-query budget (`budget_bytes`, 2 GiB, estimated at
//!   16 bytes per edge for a scan build and 8 for a transpose). Over either, the closure keeps today's seeks.
//! * A closure from one bound endpoint builds the array only once the seeks spent on that (predicate, direction,
//!   graph) in this query reach `edges / seek_divisor` (1/32): a lookup that reaches a handful of nodes stays a
//!   handful of seeks. All-pairs closures (both endpoints unbound) build it immediately.
//!
//! Order is the same as the seek BFS, row for row. That BFS emits, level by level, the first occurrence of each
//! unseen node in the concatenated `step` lists of the previous level, and a node's `step` over a link is an index
//! range (SPO `[x, p]` forward, POS `[p, x]` backward), so its neighbours come in ascending ID order. Dense order is
//! ID order, and the forward targets of a subject are its PSO row's objects in scan order (ascending); the reverse
//! targets are filled by a counting sort over ascending sources. So each node's CSR neighbour list is its `step`
//! list. Marking a node when it is pushed instead of when it is popped emits the same sequence: every node of level
//! L is pushed while level L - 1 is expanded, before any level L node is expanded, so the set a push is checked
//! against is the set the pop would have been checked against. The output is the queue itself. All-pairs keeps
//! today's start order (`+`: nodes with an outgoing step, in the order `all_pairs` of the step lists them, i.e. ID
//! order forward and (first source, ID) backward; `*`: `graph_nodes()`), runs the starts in parallel with one
//! reused bitset per rayon job, and concatenates in start order.
//!
//! Follow-up, not done here: condensing strongly connected components (one closure per component, shared by every
//! member) for all-pairs over cyclic predicates.
use super::{storage_failure, Ctx, Row, UNBOUND};
use crate::model::{Perm, TermId};
use crate::storage::Snapshot;
use crate::sparql::PropertyPathExpression as Ppe;
use crate::sparql::TermPattern;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

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
        on_data!(self, |g| for batch in g.scan(perm, prefix) {
            let b = batch.unwrap_or_else(|e| storage_failure(e));
            for i in 0..b.len() { f(b.cols[0][i], b.cols[1][i], b.cols[2][i]) }
        })
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
        let link = if matches!(p, P::Star(_) | P::Plus(_)) { simple_link(q, rev) } else { None };
        if let Some((id, dir)) = link
            && let Some(c) = self.csr(id, dir, false)
        {
            return self.csr.borrow_mut().closure(&c, x, zero);
        }
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
        // One seek per expanded node (the start, then every node emitted).
        if let Some((id, dir)) = link { self.note_seeks(id, dir, order.len() as u64 + 1) }
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
                if let Some((c, dir)) = simple_link(q, false).and_then(|(id, dir)| Some((self.csr(id, dir, true)?, dir))) {
                    c.all_pairs(&c.plus_starts(dir), false, out);
                    return;
                }
                // Start nodes are those with at least one outgoing step.
                let mut firsts = Vec::new();
                self.all_pairs(q, &mut firsts);
                let mut starts: Vec<TermId> = firsts.into_iter().map(|(s, _)| s).collect();
                dedup_keep_order(&mut starts);
                for x in starts { out.extend(self.closure(p, q, false, x).into_iter().map(|z| (x, z))) }
            }
            P::Star(q) | P::Opt(q) => {
                let csr = match p { P::Star(_) => simple_link(q, false).and_then(|(id, dir)| self.csr(id, dir, true)), _ => None };
                if let Some(c) = csr { c.all_pairs(&self.graph_nodes(), true, out); return }
                for x in self.graph_nodes() { out.extend(self.closure(p, q, false, x).into_iter().map(|z| (x, z))) }
            }
        }
    }

    /// The CSR of `p` traversed forward (`rev = false`) or backward over the active graph, or `None` to keep the
    /// seeks (`CsrCache::lookup`; `eager`: an all-pairs closure, which builds without waiting for seeks to add up).
    fn csr(&self, p: TermId, rev: bool, eager: bool) -> Option<Arc<Csr>> {
        // Exact: for a FROM union `count` is a merged scan; the cache asks for it once per key.
        self.csr.borrow_mut().lookup(CsrKey::new(p, rev, self.graph_token()), || self.count(Perm::Pso, &[p]), eager, || self.build_csr(p))
    }

    /// Records `n` seeks spent by a seek-based closure over `p` in direction `rev` (see `csr`).
    fn note_seeks(&self, p: TermId, rev: bool, n: u64) {
        self.csr.borrow_mut().note_seeks(CsrKey::new(p, rev, self.graph_token()), n)
    }

    /// Forward CSR of `p`'s edges in the active graph, from one PSO scan (rows sorted by subject, then object).
    fn build_csr(&self, p: TermId) -> Csr {
        let mut subjects: Vec<(TermId, u32)> = Vec::new(); // distinct subjects with their out-degree, ascending
        let mut objects: Vec<TermId> = Vec::new();
        self.scan_each(Perm::Pso, &[p], |_, s, o| {
            if subjects.last().is_some_and(|&(last, _)| last == s) { subjects.last_mut().unwrap().1 += 1 } else { subjects.push((s, 1)) }
            objects.push(o);
        });
        let mut nodes: Vec<TermId> = Vec::with_capacity(objects.len() + subjects.len());
        nodes.extend_from_slice(&objects);
        nodes.extend(subjects.iter().map(|&(s, _)| s));
        nodes.sort_unstable();
        nodes.dedup();
        nodes.shrink_to_fit();
        let dense = |t: TermId| nodes.binary_search(&t).expect("every edge endpoint is a node") as u32;
        let targets: Vec<u32> = objects.iter().map(|&o| dense(o)).collect();
        drop(objects);
        let mut offsets = vec![0u32; nodes.len() + 1];
        for &(s, n) in &subjects { offsets[dense(s) as usize + 1] = n }
        prefix_sum(&mut offsets);
        Csr { nodes: nodes.into(), offsets, targets }
    }

    fn is_graph_node(&self, x: TermId) -> bool {
        let n = |perm, x| on_data!(self, |g| g.count(perm, &[x])).unwrap_or_else(|e| storage_failure(e));
        n(Perm::Spo, x) > 0 || n(Perm::Osp, x) > 0
    }

    /// Every subject and object of the graph, once each (the domain of a zero-length path with no bound endpoint).
    /// Order: the subjects ascending, then the objects that are not subjects, ascending. Both key lists are sorted, so
    /// the objects are filtered by a merge against the subjects; the two lists are read side by side.
    fn graph_nodes(&self) -> Vec<TermId> {
        let (subjects, objects) = on_data!(self, |g| rayon::join(|| g.first_keys(Perm::Spo), || g.first_keys(Perm::Osp)));
        let (mut nodes, objects) = (subjects.unwrap_or_else(|e| storage_failure(e)), objects.unwrap_or_else(|e| storage_failure(e)));
        let subjects = nodes.len();
        let mut i = 0;
        for o in objects {
            while i < subjects && nodes[i] < o { i += 1 }
            if i == subjects || nodes[i] != o { nodes.push(o) }
        }
        nodes
    }

    /// The ID an endpoint has in `row`: a constant (interned, so a term absent from the store still matches itself
    /// on a zero-length path), a bound variable, or `None` if it is an unbound variable.
    fn endpoint(&self, tp: &TermPattern, row: &Row) -> Result<Option<TermId>, usize> {
        match tp {
            TermPattern::Variable(v) => { let i = self.var(v); if row[i] == UNBOUND { Err(i) } else { Ok(Some(row[i])) } }
            TermPattern::BlankNode(b) => { let i = self.vars[&format!("_:b{}", b.as_str())]; if row[i] == UNBOUND { Err(i) } else { Ok(Some(row[i])) } }
            TermPattern::NamedNode(n) => Ok(Some(self.intern(&n.clone().into()))),
            TermPattern::Literal(l) => Ok(Some(self.intern(&l.clone().into()))),
            // SPARQL 1.2: a ground triple term is a constant; any other is its hidden variable, unified with the
            // pattern once the path has bound it (`unify_path_endpoints`). `eval_path` has already returned on a ground
            // pattern that denotes no RDF term.
            #[cfg(feature = "rdf-12")]
            TermPattern::Triple(t) => match crate::triple_term::hidden_var(t) {
                Some(h) => { let i = self.vars[&h]; if row[i] == UNBOUND { Err(i) } else { Ok(Some(row[i])) } }
                None => Ok(Some(crate::triple_term::ground_term(t).map_or(UNBOUND, |term| self.intern(&term)))),
            },
        }
    }

    pub(super) fn eval_path(&self, subject: &TermPattern, path: &Ppe, object: &TermPattern, input: Vec<Row>) -> Vec<Row> {
        // A ground triple-term endpoint with a literal or triple-term subject is no RDF term: nothing matches it.
        #[cfg(feature = "rdf-12")]
        if [subject, object].iter().any(|tp| matches!(tp, TermPattern::Triple(t) if crate::triple_term::is_ground(t) && crate::triple_term::ground_term(t).is_none())) {
            return Vec::new();
        }
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
        #[cfg(feature = "rdf-12")]
        self.unify_path_endpoints([subject, object], &mut out);
        out
    }
}

/// `q` as one predicate link after peeling `^`s, with the direction a traversal in direction `rev` takes over it.
fn simple_link(q: &P, rev: bool) -> Option<(TermId, bool)> {
    match q {
        P::Link(Some(id)) => Some((*id, rev)),
        P::Rev(inner) => simple_link(inner, !rev),
        _ => None,
    }
}

/// Limits on CSR adjacencies (module docs). Tests lower them.
#[derive(Clone, Copy)]
pub(crate) struct CsrLimits {
    /// Most edges of one predicate to build an adjacency for.
    pub max_edges: u64,
    /// Estimated bytes of all adjacencies one query may build.
    pub budget_bytes: u64,
    /// A bound-endpoint closure builds once its predicate's seeks in this query reach `edges / seek_divisor`.
    pub seek_divisor: u64,
}
impl Default for CsrLimits {
    fn default() -> Self { CsrLimits { max_edges: 50_000_000, budget_bytes: 2 << 30, seek_divisor: 32 } }
}

/// An adjacency's identity within a query: predicate, direction (`rev`: backward) and active graph
/// (`Ctx::graph_token`).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CsrKey { p: TermId, rev: bool, graph: usize }
impl CsrKey {
    fn new(p: TermId, rev: bool, graph: usize) -> Self { CsrKey { p, rev, graph } }
    /// The same predicate and graph traversed the other way.
    fn reversed(self) -> Self { CsrKey { rev: !self.rev, ..self } }
}

enum CsrSlot {
    /// Not built yet: seeks spent so far by bound-endpoint closures, and the predicate's edge count.
    Seeks { done: u64, edges: u64 },
    Built(Arc<Csr>),
    /// Over a cap: always seek.
    Off,
}

/// `Ctx::csr`: the query's CSR adjacencies and the whole policy for them: whether to use one at all
/// (`Options::csr_paths`), when to build it (eagerly, or once seeks add up), the caps and the per-query budget, and
/// reusing a cached direction's transpose. Also keeps the visited bitset bound-endpoint closures share (all-zero
/// between closures).
pub(crate) struct CsrCache {
    enabled: bool,
    limits: CsrLimits,
    slots: HashMap<CsrKey, CsrSlot>,
    /// Estimated bytes of the adjacencies built so far, against `limits.budget_bytes`.
    bytes: u64,
    seen: Vec<u64>,
}
impl CsrCache {
    /// `enabled = false` (`--ablate=csrpaths`): `lookup` always answers `None`, so every closure seeks.
    pub(crate) fn new(enabled: bool, limits: CsrLimits) -> Self {
        CsrCache { enabled, limits, slots: HashMap::new(), bytes: 0, seen: Vec::new() }
    }

    /// The adjacency for `key`, or `None` to keep the seeks: disabled, over a cap, or (`eager = false`) the seeks
    /// recorded for `key` in this query (`note_seeks`) still below `edges / seek_divisor`. `edges` counts the
    /// predicate's edges and is asked once per key. `forward` scans the predicate into its forward adjacency; it is
    /// called only when the other direction is not cached (otherwise the answer is that one's transpose).
    fn lookup(&mut self, key: CsrKey, edges: impl FnOnce() -> u64, eager: bool, forward: impl FnOnce() -> Csr) -> Option<Arc<Csr>> {
        if !self.enabled { return None }
        let known = match self.slots.get(&key) {
            Some(CsrSlot::Built(c)) => return Some(c.clone()),
            Some(CsrSlot::Off) => return None,
            Some(&CsrSlot::Seeks { done, edges }) => Some((done, edges)),
            None => None,
        };
        let (done, edges) = known.unwrap_or_else(|| {
            let edges = edges();
            self.slots.insert(key, CsrSlot::Seeks { done: 0, edges });
            (0, edges)
        });
        if !eager && done < edges / self.limits.seek_divisor { return None }
        let other = match self.slots.get(&key.reversed()) { Some(CsrSlot::Built(c)) => Some(c.clone()), _ => None };
        let cost = edges.saturating_mul(if other.is_some() { 8 } else { 16 });
        if edges > self.limits.max_edges || self.bytes.saturating_add(cost) > self.limits.budget_bytes {
            self.slots.insert(key, CsrSlot::Off);
            return None;
        }
        let c = Arc::new(match other {
            Some(o) => o.transpose(),
            None => { let f = forward(); if key.rev { f.transpose() } else { f } }
        });
        self.bytes += cost;
        self.slots.insert(key, CsrSlot::Built(c.clone()));
        Some(c)
    }

    /// Records `n` seeks a seek-based closure spent on `key`, toward `lookup`'s build threshold.
    fn note_seeks(&mut self, key: CsrKey, n: u64) {
        if let Some(CsrSlot::Seeks { done, .. }) = self.slots.get_mut(&key) { *done += n }
    }

    /// The closure from `x` over `c` in discovery order (`Csr::ends`), on the shared bitset.
    fn closure(&mut self, c: &Csr, x: TermId, zero: bool) -> Vec<TermId> {
        let words = c.bitset_words();
        if self.seen.len() < words { self.seen.resize(words, 0) }
        let mut out = Vec::new();
        c.ends(x, zero, &mut self.seen[..words], &mut Vec::new(), |z| out.push(z));
        out
    }

    /// `"seeks"`, `"built"`, `"off"`, or `"none"` before the first lookup of `key`.
    #[cfg(test)]
    fn state(&self, key: CsrKey) -> &'static str {
        match self.slots.get(&key) {
            None => "none",
            Some(CsrSlot::Seeks { .. }) => "seeks",
            Some(CsrSlot::Built(_)) => "built",
            Some(CsrSlot::Off) => "off",
        }
    }
}

/// Compressed sparse row adjacency of one predicate in one direction: node `d`'s neighbours are
/// `targets[offsets[d]..offsets[d + 1]]`, ascending. Dense node numbers index `nodes`, which is sorted by ID.
pub(crate) struct Csr {
    nodes: Arc<[TermId]>,
    offsets: Vec<u32>,
    targets: Vec<u32>,
}

impl Csr {
    fn dense(&self, x: TermId) -> Option<u32> { self.nodes.binary_search(&x).ok().map(|i| i as u32) }

    /// Words of a visited bitset over the nodes.
    fn bitset_words(&self) -> usize { self.nodes.len().div_ceil(64) }

    /// Calls `emit` for the closure from `x` in discovery order (`closure_into`); `x` need not be a node: then it
    /// is its own only end for `*` and has none for `+`.
    fn ends(&self, x: TermId, zero: bool, seen: &mut [u64], queue: &mut Vec<u32>, mut emit: impl FnMut(TermId)) {
        match self.dense(x) {
            None => if zero { emit(x) },
            Some(d) => { self.closure_into(d, zero, seen, queue); for &z in queue.iter() { emit(self.nodes[z as usize]) } }
        }
    }

    fn nbrs(&self, d: u32) -> &[u32] { &self.targets[self.offsets[d as usize] as usize..self.offsets[d as usize + 1] as usize] }

    /// The same edges in the other direction; each node's new neighbours (its sources) come out ascending.
    fn transpose(&self) -> Csr {
        let n = self.nodes.len();
        let mut offsets = vec![0u32; n + 1];
        for &t in &self.targets { offsets[t as usize + 1] += 1 }
        prefix_sum(&mut offsets);
        let mut fill = offsets[..n].to_vec();
        let mut targets = vec![0u32; self.targets.len()];
        for s in 0..n as u32 {
            for &t in self.nbrs(s) {
                let f = &mut fill[t as usize];
                targets[*f as usize] = s;
                *f += 1;
            }
        }
        Csr { nodes: self.nodes.clone(), offsets, targets }
    }

    /// Fills `queue` with the closure from `x` in the seek BFS's discovery order (module docs): `x` first if `zero`
    /// (`*`), else `x` only if a cycle returns to it (`+`). `seen` must be all-zero and is left all-zero.
    fn closure_into(&self, x: u32, zero: bool, seen: &mut [u64], queue: &mut Vec<u32>) {
        fn mark(seen: &mut [u64], d: u32) -> bool {
            let (w, b) = ((d >> 6) as usize, 1u64 << (d & 63));
            let new = seen[w] & b == 0;
            seen[w] |= b;
            new
        }
        queue.clear();
        if zero { mark(seen, x); queue.push(x) }
        for &y in self.nbrs(x) { if mark(seen, y) { queue.push(y) } }
        let mut head = usize::from(zero);
        while head < queue.len() {
            let y = queue[head];
            head += 1;
            for &z in self.nbrs(y) { if mark(seen, z) { queue.push(z) } }
        }
        for &d in queue.iter() { seen[(d >> 6) as usize] &= !(1u64 << (d & 63)) }
    }

    /// Start nodes of an all-pairs `+` in the order `all_pairs` of the step would list them first: by ID for a
    /// forward link (PSO subjects); backward, a node first appears as the object of its smallest source, so by
    /// (smallest source, ID).
    fn plus_starts(&self, rev: bool) -> Vec<TermId> {
        let mut v: Vec<u32> = (0..self.nodes.len() as u32).filter(|&d| !self.nbrs(d).is_empty()).collect();
        if rev { v.sort_unstable_by_key(|&d| (self.nbrs(d)[0], d)) }
        v.into_iter().map(|d| self.nodes[d as usize]).collect()
    }

    /// (start, end) for every start in order, its closure in discovery order; starts run in parallel.
    fn all_pairs(&self, starts: &[TermId], zero: bool, out: &mut Vec<(TermId, TermId)>) {
        use rayon::prelude::*;
        let words = self.bitset_words();
        let parts: Vec<Vec<(TermId, TermId)>> = starts.par_iter().with_min_len(64)
            .map_init(|| (vec![0u64; words], Vec::new()), |(seen, queue), &x| {
                let mut v = Vec::new();
                self.ends(x, zero, seen, queue, |z| v.push((x, z)));
                v
            })
            .collect();
        out.reserve(parts.iter().map(Vec::len).sum());
        for part in parts { out.extend(part) }
    }
}

/// In place: element `k` becomes the sum of elements `0..=k` (counts at `d + 1` become offsets).
fn prefix_sum(v: &mut [u32]) {
    let mut acc = 0;
    for x in v { acc += *x; *x = acc }
}

fn dedup_keep_order(v: &mut Vec<TermId>) {
    let mut seen = HashSet::new();
    v.retain(|x| seen.insert(*x));
}

#[cfg(test)]
mod csr_tests {
    use super::*;
    use crate::bgp::sorted_probe_tests::Triples;

    const P1: u64 = 1000;
    const P2: u64 = 1001;
    /// An ID in no triple (a constant written in the query that is not in the graph).
    const ABSENT: u64 = 9999;

    /// Random graph over nodes 1..=60 with two predicates: cycles, self-loops, nodes 200..=209 only as objects, and
    /// nodes that occur only under the other predicate.
    fn graph(seed: u64) -> Vec<[u64; 3]> {
        let mut rng = seed;
        let mut next = |m: u64| { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; rng % m };
        let mut t = Vec::new();
        for _ in 0..150 { t.push([1 + next(60), P1 + next(2), 1 + next(60)]) }
        for x in 1..=8 { t.push([x, P1, x]) } // self-loops
        for x in 10..20 { t.push([x, P1, x + 1]) }
        t.push([20, P1, 10]); // a ring
        for x in 200..210 { t.push([1 + next(60), P1, x]) } // sinks
        for x in 300..305 { t.push([x, P2, 1 + next(60)]) } // nodes only under P2
        t
    }

    fn link(p: u64) -> P { P::Link(Some(TermId(p))) }
    fn paths() -> Vec<(&'static str, P)> {
        vec![
            ("p+", P::Plus(Box::new(link(P1)))),
            ("p*", P::Star(Box::new(link(P1)))),
            ("(^p)+", P::Plus(Box::new(P::Rev(Box::new(link(P1)))))),
            ("(^p)*", P::Star(Box::new(P::Rev(Box::new(link(P1)))))),
            ("^(p+)", P::Rev(Box::new(P::Plus(Box::new(link(P1)))))),
            ("q+", P::Plus(Box::new(link(P2)))),
            ("(^^q)*", P::Star(Box::new(P::Rev(Box::new(P::Rev(Box::new(link(P2)))))))),
        ]
    }

    /// Every answer shape of `path` (all pairs; each node and an absent ID as subject and as object), in order.
    fn answers(ctx: &Ctx<'_, Triples>, path: &P) -> Vec<Vec<(TermId, TermId)>> {
        let mut out = Vec::new();
        let mut all = Vec::new();
        ctx.all_pairs(path, &mut all);
        out.push(all);
        for x in (1..=60).chain(200..210).chain(300..305).chain([ABSENT]).map(TermId) {
            for rev in [false, true] {
                let mut v = Vec::new();
                ctx.step(path, rev, x, &mut v);
                out.push(v.into_iter().map(|z| (x, z)).collect());
            }
        }
        out
    }

    fn ctx(snap: &Triples, csr: bool, limits: CsrLimits) -> Ctx<'_, Triples> {
        let query = triplelight_sparql::Query::Ask {
            dataset: None,
            pattern: triplelight_sparql::GraphPattern::Bgp { patterns: Vec::new() },
            base_iri: None,
        };
        let opts = crate::Options { csr_paths: csr, range_pushdown: false, ..crate::Options::default() };
        Ctx::for_query(snap, None, None, Vec::new(), &query, &opts, limits)
    }

    /// Build at the first closure, so every bound-endpoint closure runs on the CSR.
    fn eager() -> CsrLimits { CsrLimits { seek_divisor: u64::MAX, ..CsrLimits::default() } }

    fn state(c: &Ctx<'_, Triples>, p: u64, rev: bool) -> &'static str { c.csr.borrow().state(CsrKey::new(TermId(p), rev, 0)) }
    fn built(c: &Ctx<'_, Triples>, p: u64, rev: bool) -> bool { state(c, p, rev) == "built" }

    #[test]
    fn csr_closures_match_seeks_in_order() {
        for seed in [0x2545_f491_4f6c_dd1du64, 0x9e37_79b9_7f4a_7c15, 0xdead_beef_cafe_f00d, 7] {
            let snap = Triples::new(&graph(seed));
            for (name, path) in paths() {
                let off = ctx(&snap, false, CsrLimits::default());
                // Eager limits, then the default ones (seeks first, the CSR once they add up).
                for limits in [eager(), CsrLimits::default()] {
                    let on = ctx(&snap, true, limits);
                    assert_eq!(answers(&on, &path), answers(&off, &path), "{name} seed {seed:#x}");
                }
            }
            let on = ctx(&snap, true, eager());
            answers(&on, &paths()[0].1);
            assert!(built(&on, P1, false) && built(&on, P1, true), "both directions of p+ ran on the CSR");
            let off = ctx(&snap, false, eager());
            answers(&off, &paths()[0].1);
            assert!(state(&off, P1, false) == "none" && state(&off, P1, true) == "none", "off never builds");
        }
    }

    #[test]
    fn csr_falls_back_to_seeks_over_the_caps() {
        let snap = Triples::new(&graph(3));
        let edges = snap.count(Perm::Pso, &[TermId(P1)]).unwrap();
        let small_cap = CsrLimits { max_edges: edges - 1, ..eager() };
        let small_budget = CsrLimits { budget_bytes: 16 * edges - 1, ..eager() };
        for limits in [small_cap, small_budget] {
            for (name, path) in paths() {
                let on = ctx(&snap, true, limits);
                let off = ctx(&snap, false, CsrLimits::default());
                assert_eq!(answers(&on, &path), answers(&off, &path), "{name}");
            }
            let on = ctx(&snap, true, limits);
            answers(&on, &paths()[0].1);
            assert_eq!(state(&on, P1, false), "off", "over the cap: seeks");
            assert!(!built(&on, P1, true));
        }
        // At the cap it builds.
        let on = ctx(&snap, true, CsrLimits { max_edges: edges, ..eager() });
        answers(&on, &paths()[0].1);
        assert!(built(&on, P1, false));
    }

    #[test]
    fn transpose_and_starts() {
        // 1 -> {3, 5}, 2 -> {3}, 5 -> {1}: backward, 3 first appears (as the object of 1) before 5, and 1 last.
        let snap = Triples::new(&[[1, P1, 5], [1, P1, 3], [2, P1, 3], [5, P1, 1]]);
        let c = ctx(&snap, true, eager());
        let f = c.build_csr(TermId(P1));
        let ids = |v: Vec<TermId>| v.into_iter().map(|t| t.0).collect::<Vec<_>>();
        assert_eq!(ids(f.plus_starts(false)), [1, 2, 5]);
        assert_eq!(ids(f.transpose().plus_starts(true)), [3, 5, 1]);
        let r = f.transpose();
        let back = |x: u64| r.nbrs(r.dense(TermId(x)).unwrap()).iter().map(|&d| r.nodes[d as usize].0).collect::<Vec<_>>();
        assert_eq!(back(3), [1, 2]);
        assert_eq!(back(1), [5]);
        assert!(back(2).is_empty());
    }
}
