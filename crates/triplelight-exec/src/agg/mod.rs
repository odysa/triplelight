//! Hash aggregation over term IDs (GROUP BY with COUNT / SUM / AVG / MIN / MAX / SAMPLE on variables).
//!
//! The general evaluator groups by copying every member row and then evaluates each aggregate by resolving every
//! value to an owned `Term` and folding with term arithmetic. Here groups are keyed by ID slices, each aggregate keeps a
//! small running state, and each distinct ID is decoded at most once per query (numbers and terms are memoized).
//! Semantics are those of `Ctx::aggregate`, operation for operation: same numeric typing and overflow-as-error for
//! SUM/AVG, same tie rules for MIN (first) and MAX (last) under `order_cmp`, same first-appearance group order.
//! Anything else (expressions other than a plain variable, GROUP_CONCAT, custom aggregates) uses the general path.
mod count;
mod flat;
use super::{expr, Ctx, Row, UNBOUND};
use crate::model::TermId;
use crate::storage::Snapshot;
use expr::Num;
use rayon::prelude::*;
use triplelight_rdf::{Literal, Term, Variable};
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
    /// Parallelize independent groups while retaining the serial numeric fold within each group.
    /// Evaluate single-variable expressions on the query thread, where the local term dictionary lives.
    pub(super) fn try_parallel_expression_group(
        &self,
        groups: &[(Vec<TermId>, Vec<Row>)],
        aggs: &[(Variable, AggregateExpression)],
    ) -> Option<(usize, Vec<Option<Num>>)> {
        let [(out, AggregateExpression::FunctionCall { name, expr, distinct: false })] = aggs else {
            return None;
        };
        if self.general_group || groups.len() < 2
            || !matches!(name, AggregateFunction::Sum | AggregateFunction::Avg)
            || super::impure(expr)
        {
            return None;
        }
        let variables = super::expr_vars(expr);
        let [var] = variables.as_slice() else { return None };
        let &col = self.vars.get(var)?;
        // Only large inputs amortize preparing a shared cache and dispatching workers.
        if groups.iter().map(|(_, members)| members.len()).sum::<usize>() < 16_384
            || rayon::current_num_threads() < 2
        {
            return None;
        }

        let mut values = HashMap::new();
        for row in groups.iter().flat_map(|(_, members)| members) {
            let id = row[col];
            if values.len() == super::MEMO_CAP && !values.contains_key(&id) {
                return None;
            }
            values.entry(id).or_insert_with(|| {
                self.value(expr, row).as_ref().and_then(expr::term_num)
            });
        }

        let avg = matches!(name, AggregateFunction::Avg);
        let results = groups.par_iter().map(|(_, members)| {
            let sum = members.iter().try_fold(Num::Int(0), |acc, row| {
                expr::num_add(acc, values[&row[col]]?)
            })?;
            if avg && !members.is_empty() {
                expr::num_div_count(sum, members.len() as i64)
            } else {
                Some(sum)
            }
        }).collect();
        Some((self.var(out), results))
    }

    /// GROUP BY one key with one COUNT(*) (`value_col` `None`) or COUNT(?v), over rows given as ID slices.
    fn count_one_key<'r>(&self, rows: impl IntoIterator<Item = &'r [TermId]>, key_col: usize, value_col: Option<usize>, out_col: usize) -> Vec<Row> {
        let mut positions = HashMap::new();
        let mut groups: Vec<(TermId, i64)> = Vec::new();
        let bytes_per_group = 64 + 8 + 32;
        let mut held = self.reserve(0);
        for (i, row) in rows.into_iter().enumerate() {
            if i.is_multiple_of(4096) { self.check() }
            let key = row[key_col];
            let index = *positions.entry(key).or_insert_with(|| {
                groups.push((key, 0));
                // Use the same per-group budget as one key and one generic aggregate.
                if groups.len().is_multiple_of(4096) {
                    Self::resize(&mut held, groups.len() as u64 * bytes_per_group);
                }
                groups.len() - 1
            });
            groups[index].1 += i64::from(value_col.is_none_or(|col| row[col] != UNBOUND));
        }
        Self::resize(&mut held, groups.len() as u64 * bytes_per_group);
        groups.into_iter().map(|(key, count)| {
            let mut out = self.empty_row();
            out[key_col] = key;
            out[out_col] = self.intern(&Literal::from(count).into());
            out
        }).collect()
    }

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
        if let ([key_col], [Spec { kind, col, distinct: false, out }]) = (kidx, specs.as_slice()) {
            match kind {
                Kind::CountAll { distinct: false } => return Some(self.count_one_key(rows.iter().map(Vec::as_slice), *key_col, None, *out)),
                Kind::Count => return Some(self.count_one_key(rows.iter().map(Vec::as_slice), *key_col, Some(*col), *out)),
                _ => {}
            }
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
        let mut held = self.reserve(0);
        for (n, r) in rows.iter().enumerate() {
            if n.is_multiple_of(4096) { self.check() }
            key.clear();
            key.extend(kidx.iter().map(|&i| r[i]));
            let g = match pos.get(key.as_slice()) {
                Some(&g) => g,
                None => {
                    pos.insert(key.clone(), groups.len());
                    let seen = specs.iter().map(|s| (s.distinct && !matches!(s.kind, Kind::CountAll { .. })).then(HashSet::new)).collect();
                    groups.push((key.clone(), fresh(), seen));
                    // A key, its aggregate states and a table entry per group, charged every 4096 groups.
                    if groups.len().is_multiple_of(4096) { Self::resize(&mut held, groups.len() as u64 * (64 + 8 * kidx.len() as u64 + 32 * specs.len() as u64)) }
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
