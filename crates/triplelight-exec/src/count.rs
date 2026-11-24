//! Exact aggregate pushdown for pure BGPs. Unsupported algebra keeps the ordinary evaluator.
use super::{Ctx, Row, UNBOUND, bgp, pattern, storage_failure};
use crate::{
    model::{Perm, TermId},
    storage::Snapshot,
};
use pattern::Slot;
use triplelight_rdf::{Literal, Variable};
use crate::sparql::{AggregateExpression, AggregateFunction, Expression, GraphPattern, NamedNodePattern};
use std::collections::HashSet;

#[derive(Clone, Copy)]
enum Count {
    All,
    Variable { index: Option<usize>, distinct: bool },
}
impl<S: Snapshot + ?Sized> Ctx<'_, S> {
    pub(super) fn try_count_group(
        &self,
        inner: &GraphPattern,
        keys: &[Variable],
        aggs: &[(Variable, AggregateExpression)],
        input: &[Row],
    ) -> Option<Vec<Row>> {
        if input.len() != 1 || input[0].iter().any(|&id| id != UNBOUND) || aggs.is_empty() {
            return None;
        }
        let mut requests = Vec::new();
        for (out, agg) in aggs {
            let count = match agg {
                AggregateExpression::CountSolutions { .. } => Count::All,
                AggregateExpression::FunctionCall {
                    name: AggregateFunction::Count,
                    expr: Expression::Variable(v),
                    distinct,
                } => Count::Variable {
                    index: self.vars.get(v.as_str()).copied(),
                    distinct: *distinct,
                },
                _ => return None,
            };
            requests.push((self.var(out), count));
        }
        let GraphPattern::Bgp { patterns } = inner else {
            return None;
        };
        // Triple-term patterns need per-solution unification (`triple_term`); index counts cannot see it.
        #[cfg(feature = "rdf-12")]
        if patterns.iter().any(crate::triple_term::has_triple_term) {
            return None;
        }
        // Pattern blank nodes are existential, not visible solution variables.
        // Distinct solution counting in that case needs projection-aware state.
        if aggs
            .iter()
            .any(|(_, a)| matches!(a, AggregateExpression::CountSolutions { distinct: true }))
            && patterns.iter().any(|p| {
                matches!(&p.subject, crate::sparql::TermPattern::BlankNode(_))
                    || matches!(&p.object, crate::sparql::TermPattern::BlankNode(_))
            })
        {
            return None;
        }
        if patterns.len()!=1 && (!keys.is_empty() || requests.iter().any(|(_,c)|matches!(c,Count::Variable{distinct:true,..}))) { return None; }
        let pats: Vec<[Slot; 3]> = patterns
            .iter()
            .map(|p| {
                [
                    self.slot_of(&p.subject),
                    match &p.predicate {
                        NamedNodePattern::Variable(v) => Slot::Var(self.var(v)),
                        NamedNodePattern::NamedNode(n) => {
                            Slot::Const(self.lookup(&n.clone().into()))
                        }
                    },
                    self.slot_of(&p.object),
                ]
            })
            .collect();
        // Repeated variables require diagonal tests, not just a prefix range.
        if pats.iter().any(|p| !pattern::no_repeated_var(p)) {
            return None;
        }
        let vars: HashSet<usize> = pats
            .iter()
            .flatten()
            .filter_map(|s| if let Slot::Var(v) = s { Some(*v) } else { None })
            .collect();
        let missing = pats
            .iter()
            .flatten()
            .any(|s| matches!(s, Slot::Const(None)));
        let mut tr = crate::plan_trace::enabled().then(|| crate::plan_trace::Trace::new(patterns, 1));
        if pats.len() != 1 {
            // Grouped joins and DISTINCT projections were already refused above (they need extra state).
            let (n, strategy) = if missing {
                (0, "count-empty")
            } else {
                on_data!(self, |g| {
                    let join=bgp::Bgp { snap:g,w:self.vars.len(),ranges:None,sorted_probe:self.sorted_probe };
                    if self.wcoj && pats.len()>=3 && pattern::cyclic(&pats) {
                        (join.count_leapfrog(&pats,self.variable_order(&pats)), "count-leapfrog")
                    } else {
                        let order = self.planned_order(&pats,input,&crate::range::Ranges::new());
                        if let (Some(tr), Some(o)) = (tr.as_mut(), order.as_deref()) { tr.planned(o, None) }
                        let strategy = if order.is_some() { "count-binary-planner" } else { "count-binary-greedy" };
                        (join.count_binary(&pats,order.as_deref()), strategy)
                    }
                })
            };
            if let Some(mut tr) = tr { tr.note("count", n); tr.finish(strategy, 1) }
            let mut out = self.empty_row();
            for (i, c) in requests {
                let value = match c {
                    Count::All => n,
                    Count::Variable { index, .. } => {
                        if index.is_some_and(|index| vars.contains(&index)) {
                            n
                        } else {
                            0
                        }
                    }
                };
                out[i] = self.count_literal(value);
            }
            return Some(vec![out]);
        }
        let pat = &pats[0];
        let kidx: Vec<_> = keys.iter().map(|v| self.var(v)).collect();
        let mut group_vars = Vec::new();
        for &v in &kidx {
            if vars.contains(&v) && !group_vars.contains(&v) {
                group_vars.push(v);
            }
        }
        let mut positions = [0, 1, 2];
        positions.sort_by_key(|&i| match pat[i] {
            Slot::Const(_) => (0, 0),
            Slot::Var(v) => (
                1,
                group_vars
                    .iter()
                    .position(|&x| x == v)
                    .unwrap_or(group_vars.len()),
            ),
        });
        let perm = Perm::ALL
            .into_iter()
            .find(|p| p.order() == positions)
            .unwrap();
        let mut prefix: Vec<TermId> = positions
            .iter()
            .map_while(|&i| if let Slot::Const(c) = pat[i] { c } else { None })
            .collect();
        let mut binding = self.empty_row();
        let mut out = Vec::new();
        if missing || self.count(perm, &prefix) == 0 {
            if keys.is_empty() {
                self.emit_count_group(pat, perm, &prefix, &kidx, &requests, &binding, &mut out, 0);
            }
            if let Some(tr) = tr { tr.finish("count-empty", out.len()) }
            return Some(out);
        }
        self.count_groups(
            pat,
            perm,
            &group_vars,
            0,
            &mut prefix,
            &kidx,
            &requests,
            &mut binding,
            &mut out,
        );
        if let Some(tr) = tr { tr.finish("count-single", out.len()) }
        Some(out)
    }
    fn count_literal(&self, n: u64) -> TermId {
        self.intern(&Literal::new_typed_literal(n.to_string(), triplelight_rdf::vocab::xsd::INTEGER).into())
    }
    fn count_groups(
        &self,
        pat: &[Slot; 3],
        perm: Perm,
        groups: &[usize],
        depth: usize,
        prefix: &mut Vec<TermId>,
        keys: &[usize],
        requests: &[(usize, Count)],
        binding: &mut Row,
        out: &mut Vec<Row>,
    ) {
        if depth == groups.len() {
            self.emit_count_group(
                pat,
                perm,
                prefix,
                keys,
                requests,
                binding,
                out,
                self.count(perm, prefix),
            );
            return;
        }
        let v = groups[depth];
        let mut target = TermId(1);
        while let Some(value) = on_data!(self, |g| g.seek(perm, prefix, target))
            .unwrap_or_else(|e| storage_failure(e))
        {
            prefix.push(value);
            binding[v] = value;
            self.count_groups(
                pat,
                perm,
                groups,
                depth + 1,
                prefix,
                keys,
                requests,
                binding,
                out,
            );
            prefix.pop();
            binding[v] = UNBOUND;
            if value.0 == u64::MAX {
                break;
            }
            target = TermId(value.0 + 1);
        }
    }
    fn emit_count_group(
        &self,
        pat: &[Slot; 3],
        perm: Perm,
        prefix: &[TermId],
        keys: &[usize],
        requests: &[(usize, Count)],
        binding: &Row,
        out: &mut Vec<Row>,
        n: u64,
    ) {
        let mut row = self.empty_row();
        for &k in keys {
            row[k] = binding[k];
        }
        for &(i, c) in requests {
            let value = match c {
                Count::All => n,
                Count::Variable { index: None, .. } => 0,
                Count::Variable { index: Some(index), distinct } => {
                    let position = pat
                        .iter()
                        .position(|s| matches!(s,Slot::Var(v)if *v==index));
                    match position {
                        None => 0,
                        Some(_) if !distinct => n,
                        Some(_) if n == 0 => 0,
                        Some(_) if binding[index] != UNBOUND => 1,
                        Some(_) if prefix.len() == 2 => n,
                        Some(target) => self.count_distinct(target, perm, prefix),
                    }
                }
            };
            row[i] = self.count_literal(value);
        }
        out.push(row);
    }
    fn count_distinct(
        &self,
        target: usize,
        group_perm: Perm,
        prefix: &[TermId],
    ) -> u64 {
        let mut fixed = [None; 3];
        for (key, &value) in prefix.iter().enumerate() {
            fixed[group_perm.order()[key]] = Some(value);
        }
        let mut positions = [0, 1, 2];
        positions.sort_by_key(|&i| {
            if fixed[i].is_some() {
                0
            } else if i == target {
                1
            } else {
                2
            }
        });
        let perm = Perm::ALL
            .into_iter()
            .find(|p| p.order() == positions)
            .unwrap();
        let bound: Vec<_> = positions.iter().map_while(|&i| fixed[i]).collect();
        // At the last key every value is unique in an RDF set.
        if bound.len() == 2 {
            return self.count(perm, &bound);
        }
        let mut n = 0;
        let mut next = TermId(1);
        while let Some(value) = on_data!(self, |g| g.seek(perm, &bound, next))
            .unwrap_or_else(|e| storage_failure(e))
        {
            n += 1;
            if value.0 == u64::MAX {
                break;
            }
            next = TermId(value.0 + 1);
        }
        n
    }
}
