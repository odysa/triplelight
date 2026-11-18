//! Batched evaluation for GROUP BY inputs (stage 1 of pipelined execution).
//!
//! `eval_into` delivers a pattern's solutions to a sink in consecutive batches whose concatenation is exactly what
//! `eval` returns (same rows, same order), so GROUP BY can fold them without holding the whole input. Only a few
//! shapes are batched: a BGP (its final join step runs in chunks), FILTER over a batched input, and a join whose right
//! side is batched. Anything else is evaluated as before and delivered as one batch.
//!
//! The general aggregator below folds values one at a time in input order with the operations of `Ctx::aggregate`
//! (same fold order for SUM/AVG, MIN keeps the first and MAX the last of equal values, DISTINCT keeps first
//! occurrences, errors poison the same aggregates), so its results equal the materialized path's.
use super::{conjuncts, expr, expr_vars, has_exists, Ctx, Row, UNBOUND};
use super::bgp::{self, Slot};
use crate::model::TermId;
use crate::storage::Snapshot;
use oxrdf::{Literal, Term, Variable};
use spargebra::algebra::{AggregateExpression, AggregateFunction, Expression, GraphPattern};
use spargebra::term::{NamedNodePattern, TriplePattern};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Rows per batch (of the final join step's input). Any value gives the same results; tests vary it.
static BATCH_ROWS: AtomicUsize = AtomicUsize::new(65536);

/// Sets the batch size used by batched GROUP BY evaluation (results do not depend on it).
#[doc(hidden)]
pub fn set_batch_rows(n: usize) { BATCH_ROWS.store(n.max(1), Ordering::Relaxed) }
fn batch_rows() -> usize { BATCH_ROWS.load(Ordering::Relaxed) }
static BATCHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
static NO_BATCH_ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
/// Turns batched GROUP BY evaluation on or off (off = the materialized path; `RDFDB_NO_BATCH=1` also turns it off).
#[doc(hidden)]
pub fn set_batching(on: bool) { BATCHING.store(on, Ordering::Relaxed) }

/// Whether `eval_into` batches `p` (rather than materializing it). Used to choose the GROUP BY path.
pub(super) fn batched(p: &GraphPattern) -> bool {
    if !BATCHING.load(Ordering::Relaxed) || *NO_BATCH_ENV.get_or_init(|| std::env::var_os("RDFDB_NO_BATCH").is_some()) { return false }
    match p {
        GraphPattern::Bgp { .. } => true,
        GraphPattern::Join { right, .. } => batched(right),
        GraphPattern::Filter { expr, inner } => match &**inner {
            // The equality-unification rewrite applies to FILTER over a BGP with a `?a = ?b` conjunct: keep that on
            // the materialized path.
            GraphPattern::Bgp { .. } => !conjuncts(expr).iter().any(|c| var_equality(c)),
            other => batched(other),
        },
        _ => false,
    }
}

fn var_equality(c: &Expression) -> bool {
    matches!(c, Expression::Equal(a, b) | Expression::SameTerm(a, b)
        if matches!((a.as_ref(), b.as_ref()), (Expression::Variable(_), Expression::Variable(_))))
}

impl<S: Snapshot + ?Sized> Ctx<'_, S> {
    /// Delivers `eval(p, input)` to `sink` as consecutive batches (possibly none when there are no solutions).
    pub(super) fn eval_into(&self, p: &GraphPattern, input: Vec<Row>, sink: &mut dyn FnMut(Vec<Row>)) {
        if !batched(p) { let rows = self.eval(p, input); if !rows.is_empty() { sink(rows) } return }
        match p {
            GraphPattern::Bgp { patterns } => self.bgp_into(patterns, input, &[], &[], sink),
            GraphPattern::Join { left, right } => { let l = self.eval(left, input); self.eval_into(right, l, sink) }
            GraphPattern::Filter { expr, inner } => {
                if let GraphPattern::Bgp { patterns } = &**inner {
                    // Same pushdown split as `eval`: conjuncts over BGP variables (and without EXISTS) run inside.
                    let bgp_vars: HashSet<usize> = { let mut v = Vec::new(); super::walk_vars(inner, &mut v); v.iter().filter_map(|n| self.vars.get(n).copied()).collect() };
                    let (early, late): (Vec<&Expression>, Vec<&Expression>) = conjuncts(expr).into_iter().partition(|c| {
                        let vs = expr_vars(c);
                        !vs.is_empty() && !has_exists(c) && vs.iter().all(|n| self.vars.get(n).is_some_and(|i| bgp_vars.contains(i)))
                    });
                    if !early.is_empty() {
                        let early: Vec<(&Expression, Vec<usize>)> = early.into_iter().map(|c| (c, expr_vars(c).iter().map(|n| self.vars[n]).collect())).collect();
                        return self.bgp_into(patterns, input, &early, &late, sink);
                    }
                }
                self.eval_into(inner, input, &mut |rows| {
                    let rows = match self.exists_batch(expr, &rows) { Some(out) => out, None => rows.into_iter().filter(|r| self.ebv(expr, r)).collect() };
                    if !rows.is_empty() { sink(rows) }
                })
            }
            _ => unreachable!("batched() admits only BGP, Join and Filter"),
        }
    }

    /// `eval_bgp_filtered` (then the `late` conjuncts) with its final join step run over chunks of its input.
    fn bgp_into(&self, patterns: &[TriplePattern], rows: Vec<Row>, filters: &[(&Expression, Vec<usize>)], late: &[&Expression], sink: &mut dyn FnMut(Vec<Row>)) {
        let pats: Vec<[Slot; 3]> = patterns.iter().map(|t| {
            let p = match &t.predicate {
                NamedNodePattern::Variable(v) => Slot::Var(self.var(v)),
                NamedNodePattern::NamedNode(n) => Slot::Const(self.lookup(&n.clone().into())),
            };
            [self.slot_of(&t.subject), p, self.slot_of(&t.object)]
        }).collect();
        let w = self.vars.len();
        let unbound_start = rows.len() == 1 && rows[0].iter().all(|&x| x == UNBOUND);
        let leapfrog = self.wcoj && unbound_start && pats.len() >= 3 && pats.iter().all(bgp::no_repeated_var) && bgp::cyclic(&pats);
        let finish = |mut out: Vec<Row>, sink: &mut dyn FnMut(Vec<Row>)| {
            if !late.is_empty() { out.retain(|r| late.iter().all(|c| self.ebv(c, r))) }
            if !out.is_empty() { sink(out) }
        };
        // Paths that `eval_bgp_filtered` handles specially are delivered whole.
        if rows.is_empty() || w == 0 || pats.is_empty() || leapfrog || pats.iter().any(|p| p.iter().any(|s| matches!(s, Slot::Const(None)))) {
            return finish(self.eval_bgp_filtered(patterns, rows, filters), sink);
        }
        let bgp = bgp::Bgp { snap: self.snap, w };
        let flat: Vec<TermId> = rows.concat();
        let mut applied = vec![false; filters.len()];
        let (pre, last, bound) = {
            let mut post = |r: &mut Vec<TermId>, b: &HashSet<usize>| self.apply_filters(r, w, filters, b, &mut applied);
            match self.planned_order(&pats, &rows) {
                Some(order) => bgp.eval_order_split(&pats, &order, flat, &mut post),
                None => bgp.eval_greedy_split(&pats, flat, &mut post),
            }
        };
        let Some(pi) = last else { return finish(pre.chunks_exact(w).map(<[TermId]>::to_vec).collect(), sink) };
        let mut bound = bound;
        for s in &pats[pi] { if let Slot::Var(v) = s { bound.insert(*v); } }
        for chunk in pre.chunks(w * batch_rows()) {
            let mut out = bgp.extend(&pats[pi], chunk);
            // Each chunk sees the filter state before the final step, as the whole input would.
            let mut a = applied.clone();
            self.apply_filters(&mut out, w, filters, &bound, &mut a);
            finish(out.chunks_exact(w).map(<[TermId]>::to_vec).collect(), sink);
        }
    }

    /// GROUP BY over `inner` fed in batches; same result as materializing `inner` and grouping.
    pub(super) fn group_batched(&self, inner: &GraphPattern, kidx: &[usize], aggs: &[(Variable, AggregateExpression)], input: Vec<Row>) -> Vec<Row> {
        let mut input = Some(input);
        let mut produce = |push: &mut dyn FnMut(&[Row])| {
            if let Some(i) = input.take() { self.eval_into(inner, i, &mut |b| push(&b)) }
        };
        if std::env::var_os("RDFDB_LEGACY_GROUP").is_none() {
            if let Some(out) = self.fast_group_with(kidx, aggs, &mut produce) { return out }
        }
        let specs: Vec<(usize, Option<Option<(usize, usize)>>)> = aggs.iter().map(|(v, a)| (self.var(v), match a {
            AggregateExpression::FunctionCall { expr, .. } => Some(self.agg_arg_slot(expr)),
            AggregateExpression::CountSolutions { .. } => None,
        })).collect();
        let mut groups: Vec<(Vec<TermId>, Vec<Acc>)> = Vec::new();
        let mut pos: HashMap<Vec<TermId>, usize> = HashMap::new();
        produce(&mut |rows: &[Row]| for r in rows {
            let k: Vec<TermId> = kidx.iter().map(|&i| r[i]).collect();
            let g = match pos.get(&k) {
                Some(&g) => g,
                None => { pos.insert(k.clone(), groups.len()); groups.push((k, aggs.iter().map(|(_, a)| Acc::new(a)).collect())); groups.len() - 1 }
            };
            for ((acc, (_, a)), (_, slot)) in groups[g].1.iter_mut().zip(aggs).zip(&specs) {
                match (a, slot) {
                    (AggregateExpression::FunctionCall { expr, .. }, Some(slot)) => acc.add_value(self.agg_arg_value(expr, *slot, r)),
                    _ => acc.add_row(r),
                }
            }
        });
        if groups.is_empty() && kidx.is_empty() { groups.push((vec![], aggs.iter().map(|(_, a)| Acc::new(a)).collect())); }
        groups.into_iter().map(|(k, accs)| {
            let mut out = self.empty_row();
            for (i, &ki) in kidx.iter().enumerate() { out[ki] = k[i]; }
            for (acc, (o, _)) in accs.into_iter().zip(&specs) {
                if let Some(t) = acc.finish() { out[*o] = self.intern(&t); }
            }
            out
        }).collect()
    }
}

/// Running state of one aggregate in one group: `Ctx::aggregate` folded one solution at a time.
enum Acc {
    Rows(u64),
    DistinctRows(HashSet<Row>),
    Func { name: AggregateFunction, seen: Option<HashSet<Term>>, error: bool, n: u64, sum: Option<expr::Num>, best: Option<Term>, strings: Vec<String> },
}

impl Acc {
    fn new(a: &AggregateExpression) -> Self {
        match a {
            AggregateExpression::CountSolutions { distinct: false } => Acc::Rows(0),
            AggregateExpression::CountSolutions { distinct: true } => Acc::DistinctRows(HashSet::new()),
            AggregateExpression::FunctionCall { name, distinct, .. } => Acc::Func {
                name: name.clone(), seen: distinct.then(HashSet::new), error: false, n: 0, sum: Some(expr::Num::Int(0)), best: None, strings: Vec::new(),
            },
        }
    }
    fn add_row(&mut self, r: &Row) {
        match self { Acc::Rows(n) => *n += 1, Acc::DistinctRows(s) => { s.insert(r.clone()); } Acc::Func { .. } => {} }
    }
    fn add_value(&mut self, v: Option<Term>) {
        let Acc::Func { name, seen, error, n, sum, best, strings } = self else { return };
        let Some(t) = v else { *error = true; return };
        if let Some(s) = seen { if !s.insert(t.clone()) { return } }
        *n += 1;
        match name {
            AggregateFunction::Sum | AggregateFunction::Avg => *sum = sum.and_then(|a| expr::num_add(a, expr::term_num(&t)?)),
            AggregateFunction::Min => if best.as_ref().is_none_or(|b| expr::order_cmp(Some(&t), Some(b)) == std::cmp::Ordering::Less) { *best = Some(t) },
            AggregateFunction::Max => if best.as_ref().is_none_or(|b| expr::order_cmp(Some(&t), Some(b)) != std::cmp::Ordering::Less) { *best = Some(t) },
            AggregateFunction::Sample => if best.is_none() { *best = Some(t) },
            AggregateFunction::GroupConcat { .. } => if let Some(s) = expr::string_value(&t) { strings.push(s) },
            AggregateFunction::Count | AggregateFunction::Custom(_) => {}
        }
    }
    fn finish(self) -> Option<Term> {
        match self {
            Acc::Rows(n) => Some(Literal::from(n as i64).into()),
            Acc::DistinctRows(s) => Some(Literal::from(s.len() as i64).into()),
            Acc::Func { name, error, n, sum, best, strings, .. } => {
                let strict = !matches!(name, AggregateFunction::Count | AggregateFunction::Sample);
                if strict && error { return None }
                match name {
                    AggregateFunction::Count => Some(Literal::from(n as i64).into()),
                    AggregateFunction::Sum => sum.map(|s| s.term()),
                    AggregateFunction::Avg => if n == 0 { Some(Literal::from(0i64).into()) } else { expr::num_div_count(sum?, n as i64).map(|q| q.term()) },
                    AggregateFunction::Min | AggregateFunction::Max | AggregateFunction::Sample => best,
                    AggregateFunction::GroupConcat { separator } => Some(Literal::new_simple_literal(strings.join(separator.as_deref().unwrap_or(" "))).into()),
                    AggregateFunction::Custom(_) => None,
                }
            }
        }
    }
}

