//! Query executor (owner: Kaiming-He, task #4).
//!
//! Solutions are full-width rows of `TermId` over every variable of the query; `UNBOUND` (0) marks an unbound
//! slot. Storage guarantees real IDs are never 0. Terms computed by expressions (BIND, aggregates) that are not
//! in the store get per-query scratch IDs, so ID equality stays term equality throughout.
mod expr;
pub mod results;

use crate::model::{Perm, TermId};
use crate::storage::Snapshot;
use crate::opt::{self, stats::Stats};
use oxrdf::{BlankNode, Literal, Term, Variable};
use spargebra::algebra::{AggregateExpression, AggregateFunction, Expression, GraphPattern, OrderExpression};
use spargebra::term::{GroundTerm, NamedNodePattern, TermPattern, TriplePattern};
use spargebra::Query;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

pub const UNBOUND: TermId = TermId(0);
const SCRATCH: u64 = 0xe << 60;

pub type Row = Vec<TermId>;

#[derive(Debug)]
pub enum QueryResult {
    Solutions { vars: Vec<Variable>, rows: Vec<Vec<Option<Term>>> },
    Boolean(bool),
    Graph(Vec<oxrdf::Triple>),
}

#[derive(Debug)]
pub struct QueryError(pub String);
impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}
impl std::error::Error for QueryError {}

pub fn query<S: Snapshot>(snap: &S, text: &str) -> Result<QueryResult, QueryError> {
    query_with(snap, text, &Options::default())
}

#[derive(Clone, Copy)]
pub struct Options<'a> {
    /// Statistics built from this same snapshot: BGP join order then comes from the cost-based planner (task #3);
    /// without, from a greedy exact-count heuristic.
    pub stats: Option<&'a Stats>,
    /// Use the worst-case-optimal leapfrog join for cyclic top-level BGPs.
    pub wcoj: bool,
}
impl Default for Options<'_> { fn default() -> Self { Options { stats: None, wcoj: true } } }

pub fn query_with<S: Snapshot>(snap: &S, text: &str, opts: &Options<'_>) -> Result<QueryResult, QueryError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(snap, text, opts))) {
        Ok(r) => r,
        Err(p) => match p.downcast::<StorageFailure>() {
            Ok(f) => Err(QueryError(format!("storage error: {}", f.0))),
            Err(p) => std::panic::resume_unwind(p),
        },
    }
}

fn run<S: Snapshot>(snap: &S, text: &str, opts: &Options<'_>) -> Result<QueryResult, QueryError> {
    let q = spargebra::SparqlParser::new().parse_query(text).map_err(|e| QueryError(e.to_string()))?;
    let mut ctx = Ctx::new(snap);
    ctx.wcoj = opts.wcoj;
    let stats = opts.stats;
    ctx.stats = stats.map(|st| (st, snap as &dyn Snapshot));
    match &q {
        Query::Select { pattern, .. } => {
            let (vars, pattern) = match pattern {
                GraphPattern::Project { variables, .. } => (variables.clone(), pattern),
                p => (ctx.collect_visible(p), p),
            };
            ctx.register_all(pattern);
            let rows = ctx.eval(pattern, vec![ctx.empty_row()]);
            let idx: Vec<usize> = vars.iter().map(|v| ctx.var(v)).collect();
            // Decoding IDs to terms dominates large results: do it in parallel. Scratch IDs (computed terms)
            // live in the per-query table, which is not shared across threads, so resolve those first.
            let scratch: HashMap<TermId, Term> = rows.iter().flat_map(|r| idx.iter().map(move |&i| r[i]))
                .filter(|id| id.0 >> 60 == 0xe).map(|id| (id, ctx.resolve(id))).collect();
            use rayon::prelude::*;
            let rows = rows.par_iter()
                .with_min_len(4096)
                .map(|r| idx.iter().map(|&i| {
                    let id = r[i];
                    if id == UNBOUND { None } else if let Some(t) = scratch.get(&id) { Some(t.clone()) } else { Some(snap.resolve(id)) }
                }).collect())
                .collect();
            Ok(QueryResult::Solutions { vars, rows })
        }
        Query::Ask { pattern, .. } => {
            ctx.register_all(pattern);
            let rows = ctx.eval(&GraphPattern::Slice { inner: Box::new(pattern.clone()), start: 0, length: Some(1) },
                                vec![ctx.empty_row()]);
            Ok(QueryResult::Boolean(!rows.is_empty()))
        }
        Query::Construct { template, pattern, .. } => {
            ctx.register_all(pattern);
            for t in template { ctx.register_triple(t); }
            let rows = ctx.eval(pattern, vec![ctx.empty_row()]);
            let mut out = Vec::new();
            let mut seen = HashSet::new();
            for (n, r) in rows.iter().enumerate() {
                *ctx.current.borrow_mut() = Some(r.clone());
                for t in template {
                    let inst = |tp: &TermPattern| -> Option<Term> {
                        match tp {
                            TermPattern::BlankNode(b) => Some(BlankNode::new_unchecked(format!("{}r{n}", b.as_str())).into()),
                            tp => ctx.term_slot(tp).and_then(|id| (id != UNBOUND).then(|| ctx.resolve(id))),
                        }
                    };
                    let (Some(s), Some(o)) = (inst(&t.subject), inst(&t.object)) else { continue };
                    let p = match &t.predicate {
                        NamedNodePattern::NamedNode(n) => Some(Term::NamedNode(n.clone())),
                        NamedNodePattern::Variable(v) => { let id = r[ctx.var(v)]; (id != UNBOUND).then(|| ctx.resolve(id)) }
                    };
                    let (Some(Term::NamedNode(p)), Ok(s)) = (p, oxrdf::NamedOrBlankNode::try_from(s)) else { continue };
                    let tr = oxrdf::Triple::new(s, p, o);
                    if seen.insert(tr.clone()) { out.push(tr); }
                }
            }
            Ok(QueryResult::Graph(out))
        }
        Query::Describe { .. } => Err(QueryError("DESCRIBE is not supported yet".into())),
    }
}

pub(crate) struct Ctx<'a, S: Snapshot + ?Sized> {
    pub(crate) snap: &'a S,
    vars: HashMap<String, usize>,
    scratch: RefCell<(HashMap<Term, TermId>, Vec<Term>)>,
    /// Row currently being instantiated into a CONSTRUCT template, used by `term_slot`.
    current: RefCell<Option<Row>>,
    stats: Option<(&'a Stats, &'a dyn Snapshot)>,
    wcoj: bool,
}

impl<'a, S: Snapshot + ?Sized> Ctx<'a, S> {
    fn new(snap: &'a S) -> Self {
        Ctx { snap, vars: HashMap::new(), scratch: RefCell::new((HashMap::new(), Vec::new())), current: RefCell::new(None), stats: None, wcoj: true }
    }
    fn empty_row(&self) -> Row { vec![UNBOUND; self.vars.len()] }
    fn var(&self, v: &Variable) -> usize { self.vars[v.as_str()] }
    fn var_name(&mut self, name: &str) -> usize {
        let n = self.vars.len();
        *self.vars.entry(name.to_string()).or_insert(n)
    }

    // Storage reads are fallible; an error aborts the whole query (caught in `query_with`), never yields "no rows".
    fn count(&self, perm: Perm, prefix: &[TermId]) -> u64 { self.snap.count(perm, prefix).unwrap_or_else(|e| storage_failure(e)) }
    fn seek(&self, perm: Perm, prefix: &[TermId], t: TermId) -> Option<TermId> { self.snap.seek(perm, prefix, t).unwrap_or_else(|e| storage_failure(e)) }
    fn scan(&self, perm: Perm, prefix: &[TermId]) -> impl Iterator<Item = crate::model::TripleBatch> + '_ {
        self.snap.scan(perm, prefix).map(|b| b.unwrap_or_else(|e| storage_failure(e)))
    }

    pub(crate) fn resolve(&self, id: TermId) -> Term {
        if id.0 >> 60 == 0xe { return self.scratch.borrow().1[(id.0 & !(0xf << 60)) as usize].clone(); }
        self.snap.resolve(id)
    }
    /// ID for any term: the store's ID if present, else a stable scratch ID.
    pub(crate) fn intern(&self, t: &Term) -> TermId {
        if let Some(id) = self.snap.lookup(t) { return id; }
        let mut s = self.scratch.borrow_mut();
        if let Some(&id) = s.0.get(t) { return id; }
        let id = TermId(SCRATCH | s.1.len() as u64);
        s.0.insert(t.clone(), id);
        s.1.push(t.clone());
        id
    }

    // ---------- variable registration ----------
    fn register_all(&mut self, p: &GraphPattern) {
        let mut names = Vec::new();
        walk_vars(p, &mut names);
        for n in names { self.var_name(&n); }
    }
    fn register_triple(&mut self, t: &TriplePattern) {
        for tp in [&t.subject, &t.object] { if let TermPattern::Variable(v) = tp { self.var_name(v.as_str()); } }
        if let NamedNodePattern::Variable(v) = &t.predicate { self.var_name(v.as_str()); }
    }
    fn collect_visible(&self, p: &GraphPattern) -> Vec<Variable> {
        let mut names = Vec::new();
        walk_vars(p, &mut names);
        names.into_iter().filter(|n| !n.starts_with('_')).map(Variable::new_unchecked).collect()
    }
    /// For CONSTRUCT templates: value of a template slot under the row currently being instantiated.
    fn term_slot(&self, tp: &TermPattern) -> Option<TermId> {
        match tp {
            TermPattern::Variable(v) => self.current.borrow().as_ref().map(|r| r[self.var(v)]),
            TermPattern::NamedNode(n) => Some(self.intern(&n.clone().into())),
            TermPattern::Literal(l) => Some(self.intern(&l.clone().into())),
            _ => None,
        }
    }

    // ---------- evaluation ----------
    /// Evaluates `p` for each input row (sideways information passing: bound values flow into scans).
    pub(crate) fn eval(&self, p: &GraphPattern, input: Vec<Row>) -> Vec<Row> {
        match p {
            GraphPattern::Bgp { patterns } => self.eval_bgp(patterns, input),
            GraphPattern::Join { left, right } => {
                let l = self.eval(left, input);
                self.eval(right, l)
            }
            GraphPattern::LeftJoin { left, right, expression } => {
                let l = self.eval(left, input);
                let mut out = Vec::new();
                for row in l {
                    let ext: Vec<Row> = self.eval(right, vec![row.clone()]).into_iter()
                        .filter(|r| expression.as_ref().map_or(true, |e| self.ebv(e, r))).collect();
                    if ext.is_empty() { out.push(row) } else { out.extend(ext) }
                }
                out
            }
            GraphPattern::Filter { expr, inner } => {
                self.eval(inner, input).into_iter().filter(|r| self.ebv(expr, r)).collect()
            }
            GraphPattern::Union { left, right } => {
                let mut l = self.eval(left, input.clone());
                l.extend(self.eval(right, input));
                l
            }
            GraphPattern::Extend { inner, variable, expression } => {
                let i = self.var(variable);
                self.eval(inner, input).into_iter().map(|mut r| {
                    if let Some(t) = self.value(expression, &r) { r[i] = self.intern(&t) }
                    r
                }).collect()
            }
            GraphPattern::Minus { left, right } => {
                let l = self.eval(left, input);
                let r = self.eval(right, vec![self.empty_row()]);
                l.into_iter().filter(|a| !r.iter().any(|b| {
                    let mut shared = false;
                    for (x, y) in a.iter().zip(b) {
                        if *x != UNBOUND && *y != UNBOUND { if x != y { return false } shared = true }
                    }
                    shared
                })).collect()
            }
            GraphPattern::Values { variables, bindings } => {
                let idx: Vec<usize> = variables.iter().map(|v| self.var(v)).collect();
                let mut out = Vec::new();
                for row in &input {
                    'b: for b in bindings {
                        let mut r = row.clone();
                        for (k, v) in b.iter().enumerate() {
                            let Some(v) = v else { continue };
                            let t: Term = match v { GroundTerm::NamedNode(n) => n.clone().into(), GroundTerm::Literal(l) => l.clone().into() };
                            let id = self.intern(&t);
                            if r[idx[k]] != UNBOUND && r[idx[k]] != id { continue 'b }
                            r[idx[k]] = id;
                        }
                        out.push(r);
                    }
                }
                out
            }
            GraphPattern::OrderBy { inner, expression } => {
                let rows = self.eval(inner, input);
                let mut keyed: Vec<(Vec<Option<Term>>, Row)> = rows.into_iter().map(|r| {
                    (expression.iter().map(|e| match e { OrderExpression::Asc(e) | OrderExpression::Desc(e) => self.value(e, &r) }).collect(), r)
                }).collect();
                keyed.sort_by(|(a, _), (b, _)| {
                    for (k, e) in expression.iter().enumerate() {
                        let c = expr::order_cmp(a[k].as_ref(), b[k].as_ref());
                        let c = if matches!(e, OrderExpression::Desc(_)) { c.reverse() } else { c };
                        if c.is_ne() { return c }
                    }
                    std::cmp::Ordering::Equal
                });
                keyed.into_iter().map(|(_, r)| r).collect()
            }
            GraphPattern::Project { inner, variables } => {
                let keep: HashSet<usize> = variables.iter().map(|v| self.var(v)).collect();
                // Inner variables are scoped to the subquery: evaluate from an empty row, then join back.
                let inner_rows = self.eval(inner, vec![self.empty_row()]);
                let projected: Vec<Row> = inner_rows.into_iter().map(|mut r| {
                    for (i, x) in r.iter_mut().enumerate() { if !keep.contains(&i) { *x = UNBOUND } }
                    r
                }).collect();
                join_rows(input, projected)
            }
            GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner } => {
                let mut seen = HashSet::new();
                self.eval(inner, input).into_iter().filter(|r| seen.insert(r.clone())).collect()
            }
            GraphPattern::Slice { inner, start, length } => {
                let rows = self.eval(inner, input);
                rows.into_iter().skip(*start).take(length.unwrap_or(usize::MAX)).collect()
            }
            GraphPattern::Group { inner, variables, aggregates } => self.eval_group(inner, variables, aggregates, input),
            GraphPattern::Graph { inner, .. } => self.eval(inner, input), // default graph only in P1
            GraphPattern::Path { .. } => panic!("property paths are not supported yet"),
            GraphPattern::Service { .. } => panic!("SERVICE is not supported"),
        }
    }

    fn eval_group(&self, inner: &GraphPattern, keys: &[Variable], aggs: &[(Variable, AggregateExpression)], input: Vec<Row>) -> Vec<Row> {
        let rows = self.eval(inner, input);
        let kidx: Vec<usize> = keys.iter().map(|v| self.var(v)).collect();
        let mut groups: Vec<(Vec<TermId>, Vec<Row>)> = Vec::new();
        let mut pos: HashMap<Vec<TermId>, usize> = HashMap::new();
        for r in rows {
            let k: Vec<TermId> = kidx.iter().map(|&i| r[i]).collect();
            let g = *pos.entry(k.clone()).or_insert_with(|| { groups.push((k, Vec::new())); groups.len() - 1 });
            groups[g].1.push(r);
        }
        if groups.is_empty() && keys.is_empty() { groups.push((vec![], vec![])); }
        groups.into_iter().map(|(k, members)| {
            let mut out = self.empty_row();
            for (i, &ki) in kidx.iter().enumerate() { out[ki] = k[i]; }
            for (v, agg) in aggs {
                if let Some(t) = self.aggregate(agg, &members) { out[self.var(v)] = self.intern(&t); }
            }
            out
        }).collect()
    }

    fn aggregate(&self, agg: &AggregateExpression, rows: &[Row]) -> Option<Term> {
        match agg {
            AggregateExpression::CountSolutions { distinct } => {
                let n = if *distinct { rows.iter().collect::<HashSet<_>>().len() } else { rows.len() };
                Some(Literal::from(n as i64).into())
            }
            AggregateExpression::FunctionCall { name, expr, distinct } => {
                let mut vals: Vec<Term> = rows.iter().filter_map(|r| self.value(expr, r)).collect();
                if *distinct { let mut s = HashSet::new(); vals.retain(|v| s.insert(v.clone())); }
                match name {
                    AggregateFunction::Count => Some(Literal::from(vals.len() as i64).into()),
                    AggregateFunction::Sum => vals.iter().try_fold(Term::from(Literal::from(0i64)), |a, b| expr::arith('+', &a, b)),
                    AggregateFunction::Avg => {
                        if vals.is_empty() { return Some(Literal::from(0i64).into()) }
                        let s = vals.iter().try_fold(Term::from(Literal::from(0i64)), |a, b| expr::arith('+', &a, b))?;
                        expr::arith('/', &s, &Literal::from(vals.len() as i64).into())
                    }
                    AggregateFunction::Min => vals.into_iter().min_by(|a, b| expr::order_cmp(Some(a), Some(b))),
                    AggregateFunction::Max => vals.into_iter().max_by(|a, b| expr::order_cmp(Some(a), Some(b))),
                    AggregateFunction::Sample => vals.into_iter().next(),
                    AggregateFunction::GroupConcat { separator } => {
                        let sep = separator.as_deref().unwrap_or(" ");
                        let s: Vec<String> = vals.iter().filter_map(expr::string_value).collect();
                        Some(Literal::new_simple_literal(s.join(sep)).into())
                    }
                    AggregateFunction::Custom(_) => None,
                }
            }
        }
    }

    // ---------- basic graph patterns ----------
    fn slot_of(&self, tp: &TermPattern) -> Slot {
        match tp {
            TermPattern::Variable(v) => Slot::Var(self.var(v)),
            TermPattern::BlankNode(b) => Slot::Var(self.vars[&format!("_b{}", b.as_str())]),
            TermPattern::NamedNode(n) => Slot::Const(self.snap.lookup(&n.clone().into())),
            TermPattern::Literal(l) => Slot::Const(self.snap.lookup(&l.clone().into())),
            #[allow(unreachable_patterns)]
            _ => Slot::Const(None),
        }
    }

    fn eval_bgp(&self, patterns: &[TriplePattern], mut rows: Vec<Row>) -> Vec<Row> {
        let pats: Vec<[Slot; 3]> = patterns.iter().map(|t| {
            let p = match &t.predicate {
                NamedNodePattern::Variable(v) => Slot::Var(self.var(v)),
                NamedNodePattern::NamedNode(n) => Slot::Const(self.snap.lookup(&n.clone().into())),
            };
            [self.slot_of(&t.subject), p, self.slot_of(&t.object)]
        }).collect();
        // A constant absent from the dictionary can never match.
        if pats.iter().any(|p| p.iter().any(|s| matches!(s, Slot::Const(None)))) { return Vec::new(); }
        if self.wcoj && rows.len() == 1 && pats.len() >= 3 && rows[0].iter().all(|&x| x == UNBOUND)
            && pats.iter().all(|p| no_repeated_var(p)) && cyclic(&pats) {
            return self.leapfrog(&pats, rows.pop().unwrap());
        }
        if let Some(order) = self.planned_order(&pats, &rows) {
            for pi in order { rows = self.extend(&pats[pi], rows); }
            return rows;
        }
        let mut remaining: Vec<usize> = (0..pats.len()).collect();
        // Variables bound in every input row (input rows of one call share their bound set except after OPTIONAL).
        let mut bound: HashSet<usize> = match rows.first() {
            Some(r) => (0..r.len()).filter(|&i| rows.iter().all(|x| x[i] != UNBOUND)).collect(),
            None => return rows,
        };
        while !remaining.is_empty() && !rows.is_empty() {
            // Greedy: next pattern = smallest estimated fan-out given what is bound; prefer connected patterns.
            let (k, _) = remaining.iter().enumerate().map(|(k, &pi)| (k, self.estimate(&pats[pi], &bound, rows.first().unwrap())))
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap()).unwrap();
            let pi = remaining.remove(k);
            rows = self.extend(&pats[pi], rows);
            for s in &pats[pi] { if let Slot::Var(v) = s { bound.insert(*v); } }
        }
        rows
    }

    /// Pattern order from the cost-based planner: leaves of its join tree, left to right. Used for top-level BGPs
    /// (a single all-unbound input row); nested BGPs with bound inputs keep the greedy order.
    fn planned_order(&self, pats: &[[Slot; 3]], rows: &[Row]) -> Option<Vec<usize>> {
        let (stats, dynsnap) = self.stats?;
        if pats.len() < 2 || rows.len() != 1 || rows[0].iter().any(|&x| x != UNBOUND) { return None }
        let conv = |s: &Slot| match *s { Slot::Const(c) => opt::plan::Slot::Const(c.unwrap()), Slot::Var(v) => opt::plan::Slot::Var(v as u32) };
        let tps: Vec<opt::plan::TriplePattern> = pats.iter().map(|p| opt::plan::TriplePattern { s: conv(&p[0]), p: conv(&p[1]), o: conv(&p[2]) }).collect();
        let mut st = stats.clone();
        st.cache_pattern_counts(&tps, dynsnap);
        let tree = opt::planner::plan_bgp(&tps, &st);
        fn leaves(t: &opt::plan::JoinTree, out: &mut Vec<opt::plan::TriplePattern>) {
            use opt::plan::JoinTree::*;
            match t {
                Scan { pattern, .. } => out.push(*pattern),
                IndexNestedLoop { outer, pattern, .. } => { leaves(outer, out); out.push(*pattern); }
                MergeJoin { left, right, .. } => { leaves(left, out); leaves(right, out) }
                HashJoin { build, probe, .. } => { leaves(build, out); leaves(probe, out) }
                Leapfrog { children, .. } => for c in children { leaves(c, out) },
                Unit | Empty => {}
            }
        }
        let mut ls = Vec::new();
        leaves(&tree, &mut ls);
        let mut used = vec![false; tps.len()];
        let order: Vec<usize> = ls.iter().filter_map(|l| {
            let i = (0..tps.len()).find(|&i| !used[i] && tps[i] == *l)?; used[i] = true; Some(i)
        }).collect();
        (order.len() == tps.len()).then_some(order)
    }

    /// Leapfrog triejoin (Veldhuizen, ICDT'14): bind one variable at a time; for each variable, intersect the sorted
    /// value lists of every pattern containing it, using index seeks. Each pattern reads the permutation whose key
    /// order is (its constants, then its variables in the global variable order).
    fn leapfrog(&self, pats: &[[Slot; 3]], row: Row) -> Vec<Row> {
        // Global variable order: start from the most selective pattern, then grow along shared variables,
        // preferring variables whose cheapest pattern (constants only) is smallest.
        let consts_count = |p: &[Slot; 3]| {
            let fixed: Vec<Option<TermId>> = p.iter().map(|s| if let Slot::Const(c) = s { *c } else { None }).collect();
            let (perm, prefix) = choose_perm(&fixed);
            self.count(perm, &prefix)
        };
        let counts: Vec<u64> = pats.iter().map(consts_count).collect();
        // Variable-free patterns never join on anything; they only gate the result.
        if pats.iter().zip(&counts).any(|(p, &c)| c == 0 || (p.iter().all(|s| matches!(s, Slot::Const(_))) && c == 0)) { return Vec::new() }
        let vars_of = |p: &[Slot; 3]| -> Vec<usize> { p.iter().filter_map(|s| if let Slot::Var(v) = s { Some(*v) } else { None }).collect() };
        let mut order: Vec<usize> = Vec::new();
        let all: HashSet<usize> = pats.iter().flat_map(vars_of).collect();
        while order.len() < all.len() {
            let cand = all.iter().filter(|v| !order.contains(v))
                .min_by_key(|&&v| {
                    let connected = order.is_empty() || pats.iter().any(|p| { let vs = vars_of(p); vs.contains(&v) && vs.iter().any(|w| order.contains(w)) });
                    let best = pats.iter().zip(&counts).filter(|(p, _)| vars_of(p).contains(&v)).map(|(_, c)| *c).min().unwrap_or(u64::MAX);
                    (!connected, best, v)
                }).copied().unwrap();
            order.push(cand);
        }
        let rank = |v: usize| order.iter().position(|&w| w == v).unwrap();
        // Per pattern: its permutation and, per position in that key order, what fills it.
        struct P { perm: Perm, key: [Slot; 3] }
        let plans: Vec<P> = pats.iter().map(|p| {
            let mut pos = [0usize, 1, 2];
            pos.sort_by_key(|&i| match p[i] { Slot::Const(_) => (0, 0), Slot::Var(v) => (1, rank(v)) });
            let perm = *Perm::ALL.iter().find(|q| q.order() == pos).unwrap();
            P { perm, key: [p[pos[0]], p[pos[1]], p[pos[2]]] }
        }).collect();
        let mut out = Vec::new();
        let mut row = row;
        self.lf_depth(&plans.iter().map(|p| (p.perm, p.key)).collect::<Vec<_>>(), &order, 0, &mut row, &mut out);
        out
    }

    fn lf_depth(&self, plans: &[(Perm, [Slot; 3])], order: &[usize], d: usize, row: &mut Row, out: &mut Vec<Row>) {
        if d == order.len() { out.push(row.clone()); return }
        let v = order[d];
        // For each pattern containing v: key prefix (constants + already-bound variables) before v.
        let mut its: Vec<(Perm, Vec<TermId>)> = Vec::new();
        for (perm, key) in plans {
            let Some(k) = key.iter().position(|s| matches!(s, Slot::Var(w) if *w == v)) else { continue };
            let prefix: Vec<TermId> = key[..k].iter().map(|s| match s { Slot::Const(c) => c.unwrap(), Slot::Var(w) => row[*w] }).collect();
            its.push((*perm, prefix));
        }
        let mut x = TermId(1);
        'outer: loop {
            let mut agreed = 0;
            let mut i = 0;
            while agreed < its.len() {
                let (perm, prefix) = &its[i];
                let Some(y) = self.seek(*perm, prefix, x) else { break 'outer };
                if y == x { agreed += 1 } else { x = y; agreed = 1 }
                i = (i + 1) % its.len();
            }
            row[v] = x;
            self.lf_depth(plans, order, d + 1, row, out);
            row[v] = UNBOUND;
            if x.0 == u64::MAX { break }
            x = TermId(x.0 + 1);
        }
    }

    fn estimate(&self, pat: &[Slot; 3], bound: &HashSet<usize>, sample: &Row) -> f64 {
        let fixed: Vec<Option<TermId>> = pat.iter().map(|s| match s {
            Slot::Const(c) => *c,
            Slot::Var(v) if bound.contains(v) => Some(sample[*v]),
            _ => None,
        }).collect();
        let (perm, prefix) = choose_perm(&fixed);
        let n = self.count(perm, &prefix) as f64;
        let connected = pat.iter().any(|s| matches!(s, Slot::Var(v) if bound.contains(v)));
        if bound.is_empty() || connected { n } else { n * 1e6 } // avoid cross products
    }

    /// Index nested-loop join of `rows` with one triple pattern, reading only the matching index range per row.
    fn extend(&self, pat: &[Slot; 3], rows: Vec<Row>) -> Vec<Row> {
        let mut out = Vec::new();
        for row in rows {
            let fixed: Vec<Option<TermId>> = pat.iter().map(|s| match s {
                Slot::Const(c) => *c,
                Slot::Var(v) => (row[*v] != UNBOUND).then_some(row[*v]),
            }).collect();
            let (perm, prefix) = choose_perm(&fixed);
            // Fully bound pattern: a membership probe, no scan or batch allocation.
            if prefix.len() == 3 {
                if self.count(perm, &prefix) > 0 { out.push(row) }
                continue;
            }
            let order = perm.order();
            for batch in self.scan(perm, &prefix) {
                'rows: for i in 0..batch.len() {
                    let mut spo = [UNBOUND; 3];
                    for (k, &pos) in order.iter().enumerate() { spo[pos] = batch.cols[k][i]; }
                    let mut r = row.clone();
                    for pos in 0..3 {
                        if let Slot::Var(v) = pat[pos] {
                            if r[v] == UNBOUND { r[v] = spo[pos] } else if r[v] != spo[pos] { continue 'rows }
                        }
                    }
                    out.push(r);
                }
            }
        }
        out
    }

    pub(crate) fn ebv(&self, e: &Expression, r: &Row) -> bool {
        self.value(e, r).and_then(|t| expr::ebv(&t)).unwrap_or(false)
    }
    pub(crate) fn value(&self, e: &Expression, r: &Row) -> Option<Term> { expr::eval(self, e, r) }
    pub(crate) fn var_value(&self, v: &Variable, r: &Row) -> Option<Term> {
        let id = *r.get(*self.vars.get(v.as_str())?)?;
        (id != UNBOUND).then(|| self.resolve(id))
    }
    pub(crate) fn exists(&self, p: &GraphPattern, r: &Row) -> bool { !self.eval(p, vec![r.clone()]).is_empty() }
}

/// Marker payload for storage failures raised inside evaluation.
struct StorageFailure(String);
fn storage_failure(e: crate::storage::StorageError) -> ! { std::panic::panic_any(StorageFailure(e.to_string())) }

#[derive(Copy, Clone)]
enum Slot { Const(Option<TermId>), Var(usize) }

/// GYO reduction: a BGP is acyclic iff repeatedly removing variables that occur in one pattern only, and patterns
/// whose variables are contained in another pattern's, empties the hypergraph.
fn cyclic(pats: &[[Slot; 3]]) -> bool {
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

fn no_repeated_var(p: &[Slot; 3]) -> bool {
    let vs: Vec<usize> = p.iter().filter_map(|s| if let Slot::Var(v) = s { Some(*v) } else { None }).collect();
    vs.iter().collect::<HashSet<_>>().len() == vs.len()
}

/// Picks the permutation whose leading key components are exactly the fixed positions.
fn choose_perm(fixed: &[Option<TermId>]) -> (Perm, Vec<TermId>) {
    let b = [fixed[0].is_some(), fixed[1].is_some(), fixed[2].is_some()];
    let perm = match b {
        [true, true, _] => Perm::Spo,
        [true, false, true] => Perm::Sop,
        [true, false, false] => Perm::Spo,
        [false, true, true] => Perm::Pos,
        [false, true, false] => Perm::Pso,
        [false, false, true] => Perm::Osp,
        [false, false, false] => Perm::Spo,
    };
    let prefix = perm.order().iter().map_while(|&p| fixed[p]).collect();
    (perm, prefix)
}

/// Compatible-mapping join (used to merge subquery results back into the outer rows).
fn join_rows(left: Vec<Row>, right: Vec<Row>) -> Vec<Row> {
    let mut out = Vec::new();
    for a in &left {
        'r: for b in &right {
            let mut r = a.clone();
            for (i, &y) in b.iter().enumerate() {
                if y == UNBOUND { continue }
                if r[i] == UNBOUND { r[i] = y } else if r[i] != y { continue 'r }
            }
            out.push(r);
        }
    }
    out
}

fn walk_vars(p: &GraphPattern, out: &mut Vec<String>) {
    let mut add = |s: String| if !out.contains(&s) { out.push(s) };
    match p {
        GraphPattern::Bgp { patterns } => for t in patterns {
            for tp in [&t.subject, &t.object] {
                match tp {
                    TermPattern::Variable(v) => add(v.as_str().to_string()),
                    TermPattern::BlankNode(b) => add(format!("_b{}", b.as_str())),
                    _ => {}
                }
            }
            if let NamedNodePattern::Variable(v) = &t.predicate { add(v.as_str().to_string()) }
        },
        GraphPattern::Path { subject, object, .. } => for tp in [subject, object] {
            if let TermPattern::Variable(v) = tp { add(v.as_str().to_string()) }
        },
        GraphPattern::Join { left, right } | GraphPattern::Union { left, right } | GraphPattern::Minus { left, right }
        => { walk_vars(left, out); walk_vars(right, out) }
        GraphPattern::LeftJoin { left, right, expression } => {
            walk_vars(left, out); walk_vars(right, out);
            if let Some(e) = expression { walk_expr(e, out) }
        }
        GraphPattern::Filter { expr, inner } => { walk_vars(inner, out); walk_expr(expr, out) }
        GraphPattern::Extend { inner, variable, expression } => {
            walk_vars(inner, out); walk_expr(expression, out);
            let s = variable.as_str().to_string(); if !out.contains(&s) { out.push(s) }
        }
        GraphPattern::Values { variables, .. } => for v in variables { add(v.as_str().to_string()) },
        GraphPattern::OrderBy { inner, .. } | GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. } | GraphPattern::Graph { inner, .. } | GraphPattern::Service { inner, .. } => walk_vars(inner, out),
        GraphPattern::Project { inner, variables } => {
            walk_vars(inner, out);
            for v in variables { let s = v.as_str().to_string(); if !out.contains(&s) { out.push(s) } }
        }
        GraphPattern::Group { inner, variables, aggregates } => {
            walk_vars(inner, out);
            for v in variables { let s = v.as_str().to_string(); if !out.contains(&s) { out.push(s) } }
            for (v, _) in aggregates { let s = v.as_str().to_string(); if !out.contains(&s) { out.push(s) } }
        }
    }
}

fn walk_expr(e: &Expression, out: &mut Vec<String>) {
    match e {
        Expression::Exists(p) => walk_vars(p, out),
        Expression::Variable(v) | Expression::Bound(v) => { let s = v.as_str().to_string(); if !out.contains(&s) { out.push(s) } }
        Expression::Or(a, b) | Expression::And(a, b) | Expression::Equal(a, b) | Expression::SameTerm(a, b)
        | Expression::Greater(a, b) | Expression::GreaterOrEqual(a, b) | Expression::Less(a, b) | Expression::LessOrEqual(a, b)
        | Expression::Add(a, b) | Expression::Subtract(a, b) | Expression::Multiply(a, b) | Expression::Divide(a, b) => { walk_expr(a, out); walk_expr(b, out) }
        Expression::UnaryPlus(a) | Expression::UnaryMinus(a) | Expression::Not(a) => walk_expr(a, out),
        Expression::In(a, l) => { walk_expr(a, out); for x in l { walk_expr(x, out) } }
        Expression::If(a, b, c) => { walk_expr(a, out); walk_expr(b, out); walk_expr(c, out) }
        Expression::Coalesce(l) | Expression::FunctionCall(_, l) => for x in l { walk_expr(x, out) },
        Expression::NamedNode(_) | Expression::Literal(_) => {}
    }
}
