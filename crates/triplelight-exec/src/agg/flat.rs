//! GROUP BY one key with one COUNT straight over a BGP's flat result buffer (`Ctx::eval_bgp_flat`), e.g. BSBM BI q3's
//! `{ ?review bsbm:reviewFor ?product . ?review dc:date ?date FILTER(...) } GROUP BY ?product` with `COUNT(?review)`.
//! The general route turns every solution into its own `Row` (one allocation each) only for `count_one_key` to read it
//! once; here the rows are read in place, in the same order, so the groups, their order and their counts are the same.
use super::super::{conjuncts, phase, plan_trace, triple_term_free, Ctx, Row};
use crate::sparql::{AggregateExpression, AggregateFunction, Expression, GraphPattern};
use crate::storage::Snapshot;
use triplelight_rdf::Variable;

impl<S: Snapshot + ?Sized> Ctx<'_, S> {
    /// The grouped rows if `inner` is a BGP, or a FILTER over one whose conjuncts all run inside it, grouped by one
    /// variable with one non-DISTINCT `COUNT(*)` or `COUNT(?v)`; `Err(input)` (unchanged) otherwise.
    pub(crate) fn try_flat_count_group(&self, inner: &GraphPattern, keys: &[Variable], aggs: &[(Variable, AggregateExpression)], input: Vec<Row>) -> Result<Vec<Row>, Vec<Row>> {
        if self.general_group || self.vars.is_empty() { return Err(input) }
        let ([key], [(out, agg)]) = (keys, aggs) else { return Err(input) };
        let value_col = match agg {
            AggregateExpression::CountSolutions { distinct: false } => None,
            AggregateExpression::FunctionCall { name: AggregateFunction::Count, expr: Expression::Variable(v), distinct: false } => match self.vars.get(v.as_str()) {
                Some(&c) => Some(c),
                None => return Err(input),
            },
            _ => return Err(input),
        };
        let (patterns, filter) = match inner {
            GraphPattern::Bgp { patterns } => (patterns, None),
            GraphPattern::Filter { expr, inner } => match inner.as_ref() {
                GraphPattern::Bgp { patterns } => (patterns, Some(expr)),
                _ => return Err(input),
            },
            _ => return Err(input),
        };
        if !triple_term_free(patterns) { return Err(input) }
        let early = match filter {
            None => Vec::new(),
            Some(expr) => {
                // `eval_filter` rewrites `?a = ?b` over non-literals into a join; leave those to it.
                if conjuncts(expr).iter().any(|c| self.unifiable_equality(c, patterns).is_some()) { return Err(input) }
                let (early, late) = self.pushed_filters(expr, inner);
                if early.is_empty() || !late.is_empty() { return Err(input) }
                early
            }
        };
        let (key_col, out_col, w) = (self.var(key), self.var(out), self.vars.len());
        self.check();
        // The flat COUNT route bypasses `eval` for both the Filter and BGP, so record those observed children here.
        let filter_operator = filter.map(|_| plan_trace::operator(inner, input.len()));
        let bgp_pattern = match inner { GraphPattern::Filter { inner, .. } => inner.as_ref(), _ => inner };
        let bgp_operator = plan_trace::operator(bgp_pattern, input.len());
        let flat = self.eval_bgp_filtered_flat(patterns, input, &early);
        let rows_out = flat.len() / w;
        bgp_operator.finish(rows_out);
        if let Some(operator) = filter_operator { operator.finish(rows_out); }
        // The same charge as the BGP's materialized rows (`Ctx::eval`).
        self.fits(rows_out as u64 * self.row_bytes());
        // Counted in memory unless that runs out of budget; then one key partition at a time from disk (#111).
        if !self.forced_spill(rows_out)
            && let Some(out) = phase::group(|| self.in_memory_or_spill(true, || self.count_one_key(flat.chunks_exact(w), key_col, value_col, out_col))) {
            return Ok(out);
        }
        let spilled = self.spill_for_group(&[key_col], rows_out, flat.chunks_exact(w));
        drop(flat);
        Ok(phase::group(|| self.group_partitions(&spilled, &[key_col], |part| self.count_one_key(part.iter().map(Vec::as_slice), key_col, value_col, out_col))))
    }
}
