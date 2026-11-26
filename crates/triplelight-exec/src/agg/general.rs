//! Term-level aggregation for expressions and groups outside the ID fast paths.
use crate::sparql::{AggregateExpression, AggregateFunction, Expression};
use crate::storage::Snapshot;
use crate::{Ctx, MEMO_CAP, Row, expr, expr_vars, impure};
use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use triplelight_rdf::{Literal, Term};

impl<S: Snapshot + ?Sized> Ctx<'_, S> {
    pub(crate) fn aggregate(&self, agg: &AggregateExpression, rows: &[Row]) -> Option<Term> {
        match agg {
            AggregateExpression::CountSolutions { distinct } => {
                let n = if *distinct {
                    rows.iter().collect::<HashSet<_>>().len()
                } else {
                    rows.len()
                };
                Some(Literal::from(n as i64).into())
            }
            AggregateExpression::FunctionCall {
                name: AggregateFunction::Custom(iri),
                expr,
                distinct,
            } => self.aggregate_custom(iri.as_str(), expr, *distinct, rows),
            AggregateExpression::FunctionCall {
                name,
                expr,
                distinct,
            } => {
                // SPARQL 1.1 §18.5: the expression is evaluated per solution; an error (including an unbound
                // variable) makes SUM/AVG/MIN/MAX/GROUP_CONCAT an error (unbound result). COUNT counts the non-error
                // values; SAMPLE returns a non-error value. (Matches Jena.)
                // Impure calls may differ across rows and EXISTS can read other variables.
                let single = match expr_vars(expr).as_slice() {
                    [v] if !impure(expr) => self.vars.get(v).copied(),
                    _ => None,
                };
                let evaluated: Vec<Option<Term>> = match single {
                    Some(vi) => {
                        let e: &Expression = expr;
                        let addr = e as *const Expression as usize;
                        let eid = {
                            let mut m = self.agg_arg_slots.borrow_mut();
                            match m.get(&addr) {
                                Some((q, slot)) if q == e => *slot,
                                _ => {
                                    let slot = self.memo_slots.get();
                                    self.memo_slots.set(slot + 1);
                                    m.insert(addr, (e.clone(), slot));
                                    slot
                                }
                            }
                        };
                        rows.iter()
                            .map(|r| {
                                let key = (eid, r[vi]);
                                if let Some(v) = self.agg_arg_memo.borrow().get(&key) {
                                    return v.clone();
                                }
                                let v = self.value(expr, r);
                                let mut m = self.agg_arg_memo.borrow_mut();
                                if m.len() < MEMO_CAP {
                                    m.insert(key, v.clone());
                                }
                                v
                            })
                            .collect()
                    }
                    None => rows.iter().map(|r| self.value(expr, r)).collect(),
                };
                let has_error = evaluated.iter().any(Option::is_none);
                let mut vals: Vec<Term> = evaluated.into_iter().flatten().collect();
                if *distinct {
                    let mut s = HashSet::new();
                    vals.retain(|v| s.insert(v.clone()));
                }
                let strict = !matches!(name, AggregateFunction::Count | AggregateFunction::Sample);
                if strict && has_error {
                    return None;
                }
                match name {
                    AggregateFunction::Count => Some(Literal::from(vals.len() as i64).into()),
                    // Same accumulation as the ID-level fast path (`expr::num_add`): float sums round once.
                    AggregateFunction::Sum => vals
                        .iter()
                        .try_fold(expr::Num::Int(0), |a, b| {
                            expr::num_add(a, expr::term_num(b)?)
                        })
                        .map(|n| n.term()),
                    AggregateFunction::Avg => {
                        if vals.is_empty() {
                            return Some(Literal::from(0i64).into());
                        }
                        let s = vals.iter().try_fold(expr::Num::Int(0), |a, b| {
                            expr::num_add(a, expr::term_num(b)?)
                        })?;
                        expr::num_div_count(s, vals.len() as i64).map(|n| n.term())
                    }
                    AggregateFunction::Min => vals
                        .into_iter()
                        .min_by(|a, b| expr::order_cmp(Some(a), Some(b))),
                    AggregateFunction::Max => vals
                        .into_iter()
                        .max_by(|a, b| expr::order_cmp(Some(a), Some(b))),
                    AggregateFunction::Sample => vals.into_iter().next(),
                    AggregateFunction::GroupConcat { separator } => {
                        let sep = separator.as_deref().unwrap_or(" ");
                        let s: Vec<String> = vals.iter().filter_map(expr::string_value).collect();
                        Some(Literal::new_simple_literal(s.join(sep)).into())
                    }
                    AggregateFunction::Custom(_) => {
                        unreachable!("custom aggregates use the streaming fold")
                    }
                }
            }
        }
    }

    fn aggregate_custom(
        &self,
        iri: &str,
        expr: &Expression,
        distinct: bool,
        rows: &[Row],
    ) -> Option<Term> {
        let factory = self.functions?.aggregate(iri)?;
        let mut state = self.call_aggregate(|| factory.create())??;
        let mut seen = HashSet::new();
        let mut held = self.reserve(0);
        let mut bytes = 0u64;
        for row in rows {
            self.check();
            let Some(term) = self.value(expr, row) else {
                continue;
            };
            if distinct && seen.contains(&term) {
                continue;
            }
            if distinct && held.is_some() {
                let estimate = (2 * std::mem::size_of::<Term>() + term.to_string().len()) as u64;
                bytes = bytes.saturating_add(estimate);
                Self::resize(&mut held, bytes);
            }
            self.call_aggregate(|| state.step(&term))??;
            if distinct {
                seen.insert(term);
            }
        }
        self.call_aggregate(|| state.finish())?
    }

    fn call_aggregate<T>(&self, callback: impl FnOnce() -> T) -> Option<T> {
        self.check();
        let result = catch_unwind(AssertUnwindSafe(callback));
        self.check();
        result.ok()
    }
}
