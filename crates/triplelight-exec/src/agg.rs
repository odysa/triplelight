//! Hash aggregation over term IDs (GROUP BY with COUNT / SUM / AVG / MIN / MAX / SAMPLE on variables).
//!
//! The general evaluator groups by copying every member row and then evaluates each aggregate by resolving every
//! value to an owned `Term` and folding with term arithmetic. Here groups are keyed by ID slices, each aggregate keeps a
//! small running state, and each distinct ID is decoded at most once per query (numbers and terms are memoized).
//! Semantics are those of `Ctx::aggregate`, operation for operation: same numeric typing and overflow-as-error for
//! SUM/AVG, same tie rules for MIN (first) and MAX (last) under `order_cmp`, same first-appearance group order.
//! Anything else (expressions other than a plain variable, GROUP_CONCAT, custom aggregates) uses the general path.
use super::{expr, Ctx, Row, UNBOUND};
use crate::model::TermId;
use crate::storage::Snapshot;
use expr::Num;
use oxrdf::{Literal, Term, Variable};
use crate::sparql::{AggregateExpression, AggregateFunction, Expression};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy)]
enum Kind { CountAll { distinct: bool }, Count, Sum, Avg, Min, Max, Sample }

struct Spec { kind: Kind, col: usize, distinct: bool, out: usize }

enum State {
    CountAll(u64, Option<HashSet<Vec<TermId>>>),
    Count(u64),
    /// Running sum; `None` once a SPARQL error occurred (non-numeric value or integer overflow).
    Sum(Option<Num>, u64),
    Best(Option<TermId>),
}

impl<S: Snapshot + ?Sized> Ctx<'_, S> {
    /// Fast GROUP BY over already evaluated rows; `None` if some aggregate isn't supported here.
    pub(super) fn try_fast_group(&self, rows: &[Row], kidx: &[usize], aggs: &[(Variable, AggregateExpression)]) -> Option<Vec<Row>> {
        let mut specs = Vec::with_capacity(aggs.len());
        for (v, a) in aggs {
            let out = self.var(v);
            specs.push(match a {
                AggregateExpression::CountSolutions { distinct } => Spec { kind: Kind::CountAll { distinct: *distinct }, col: 0, distinct: *distinct, out },
                AggregateExpression::FunctionCall { name, expr: Expression::Variable(x), distinct } => {
                    let col = *self.vars.get(x.as_str())?;
                    let kind = match name {
                        AggregateFunction::Count => Kind::Count,
                        AggregateFunction::Sum => Kind::Sum,
                        AggregateFunction::Avg => Kind::Avg,
                        AggregateFunction::Min => Kind::Min,
                        AggregateFunction::Max => Kind::Max,
                        AggregateFunction::Sample => Kind::Sample,
                        _ => return None,
                    };
                    Spec { kind, col, distinct: *distinct, out }
                }
                _ => return None,
            });
        }
        let nums: RefCell<HashMap<TermId, Option<Num>>> = RefCell::new(HashMap::new());
        let terms: RefCell<HashMap<TermId, Term>> = RefCell::new(HashMap::new());
        let num = |id: TermId| *nums.borrow_mut().entry(id).or_insert_with(|| expr::term_num(&self.resolve(id)));
        let cmp = |a: TermId, b: TermId| -> Ordering {
            let mut t = terms.borrow_mut();
            let ta = t.entry(a).or_insert_with(|| self.resolve(a)).clone();
            let tb = t.entry(b).or_insert_with(|| self.resolve(b));
            expr::order_cmp(Some(&ta), Some(tb))
        };
        let fresh = || -> Vec<State> {
            specs.iter().map(|s| match s.kind {
                Kind::CountAll { distinct } => State::CountAll(0, distinct.then(HashSet::new)),
                Kind::Count => State::Count(0),
                Kind::Sum | Kind::Avg => State::Sum(Some(Num::Int(0)), 0),
                Kind::Min | Kind::Max | Kind::Sample => State::Best(None),
            }).collect()
        };
        // Per group and aggregate: the distinct values already folded (only for DISTINCT aggregates over a variable).
        let mut groups: Vec<(Vec<TermId>, Vec<State>, Vec<Option<HashSet<TermId>>>)> = Vec::new();
        let mut pos: HashMap<Vec<TermId>, usize> = HashMap::new();
        let mut key: Vec<TermId> = Vec::with_capacity(kidx.len());
        // MIN/MAX per (group, aggregate) that met an error.
        let mut poisoned: Vec<bool> = Vec::new();
        for r in rows {
            key.clear();
            key.extend(kidx.iter().map(|&i| r[i]));
            let g = match pos.get(key.as_slice()) {
                Some(&g) => g,
                None => {
                    pos.insert(key.clone(), groups.len());
                    let seen = specs.iter().map(|s| (s.distinct && !matches!(s.kind, Kind::CountAll { .. })).then(HashSet::new)).collect();
                    groups.push((key.clone(), fresh(), seen));
                    poisoned.extend(std::iter::repeat_n(false, specs.len()));
                    groups.len() - 1
                }
            };
            let (_, states, seen) = &mut groups[g];
            for (i, s) in specs.iter().enumerate() {
                if let State::CountAll(n, set) = &mut states[i] {
                    match set { Some(set) => { set.insert(r.clone()); } None => *n += 1 }
                    continue;
                }
                let id = r[s.col];
                if id == UNBOUND {
                    // An unbound value is an evaluation error: it poisons SUM/AVG/MIN/MAX (COUNT and SAMPLE skip it).
                    match (&mut states[i], s.kind) {
                        (State::Sum(acc, _), _) => *acc = None,
                        (State::Best(_), Kind::Min | Kind::Max) => poisoned[g * specs.len() + i] = true,
                        _ => {}
                    }
                    continue;
                }
                if let Some(set) = &mut seen[i] && !set.insert(id) { continue }
                match (&mut states[i], s.kind) {
                    (State::Count(n), _) => *n += 1,
                    (State::Sum(acc, n), _) => {
                        *n += 1;
                        *acc = match (*acc, num(id)) { (Some(a), Some(x)) => expr::num_add(a, x), _ => None };
                    }
                    (State::Best(best), Kind::Min) => if best.is_none_or(|b| cmp(id, b) == Ordering::Less) { *best = Some(id) },
                    (State::Best(best), Kind::Max) => if best.is_none_or(|b| cmp(id, b) != Ordering::Less) { *best = Some(id) },
                    (State::Best(best), _) if best.is_none() => { *best = Some(id) },
                    _ => {}
                }
            }
        }
        if groups.is_empty() && kidx.is_empty() {
            groups.push((vec![], fresh(), specs.iter().map(|_| None).collect()));
            poisoned.extend(std::iter::repeat_n(false, specs.len()));
        }
        let nspec = specs.len();
        Some(groups.into_iter().enumerate().map(|(gi, (k, states, _))| {
            let mut out = self.empty_row();
            for (i, &ki) in kidx.iter().enumerate() { out[ki] = k[i]; }
            for (si, (s, st)) in specs.iter().zip(states).enumerate() {
                let v: Option<TermId> = match (st, s.kind) {
                    (State::CountAll(n, set), _) => Some(self.intern(&Literal::from(set.map_or(n, |s| s.len() as u64) as i64).into())),
                    (State::Count(n), _) => Some(self.intern(&Literal::from(n as i64).into())),
                    (State::Sum(acc, _), Kind::Sum) => acc.map(|a| self.intern(&a.term())),
                    (State::Sum(acc, n), _) => {
                        // AVG: 0 for an empty group, else sum / n with `arith('/')` typing.
                        if acc.is_none() { None }
                        else if n == 0 { Some(self.intern(&Literal::from(0i64).into())) }
                        else { acc.and_then(|a| expr::num_div_count(a, n as i64)).map(|q| self.intern(&q.term())) }
                    }
                    (State::Best(_), Kind::Min | Kind::Max) if poisoned[gi * nspec + si] => None,
                    (State::Best(b), _) => b,
                };
                if let Some(v) = v { out[s.out] = v }
            }
            out
        }).collect())
    }
}
