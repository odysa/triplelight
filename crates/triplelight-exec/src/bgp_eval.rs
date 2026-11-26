//! Flat BGP execution and optional wide-table access.
use super::*;

impl<'a, S: Snapshot + ?Sized> Ctx<'a, S> {
    /// `eval_bgp_slots` for a query with at least one variable, as one flat buffer of rows (`vars.len()` IDs each),
    /// so a consumer that reads the rows once (`agg::flat`) skips building a `Row` per solution.
    pub(super) fn eval_bgp_flat(
        &self,
        patterns: &[TriplePattern],
        rows: Vec<Row>,
        filters: &[(&Expression, Vec<usize>)],
    ) -> Vec<TermId> {
        let pats: Vec<[Slot; 3]> = patterns.iter().map(|t| self.slots(t)).collect();
        let mut tr = plan_trace::enabled().then(|| plan_trace::Trace::new(patterns, rows.len()));
        // A constant absent from the dictionary can never match.
        if rows.is_empty()
            || pats
                .iter()
                .any(|p| p.iter().any(|s| matches!(s, Slot::Const(None))))
        {
            if let Some(tr) = tr {
                tr.finish("empty", 0)
            }
            return Vec::new();
        }
        let w = self.vars.len();
        let unbound_start = rows.len() == 1 && rows[0].iter().all(|&x| x == UNBOUND);
        let ranges = self.object_ranges(patterns, &pats, filters, &rows[0]);
        let flat: Vec<TermId> = rows.concat();
        // Numeric literals often flow through multiple range predicates in the same BGP. Keep decoded
        // values for the whole BGP instead of resolving and parsing them again for every predicate.
        let mut numeric_filter_memo: HashMap<TermId, Option<expr::Num>> = HashMap::new();
        if let Some(tr) = tr.as_mut() {
            tr.note("ranges", ranges.len());
            tr.note("filters", filters.len())
        }
        let (out, strategy) = on_data!(self, |g| {
            let bgp = bgp::Bgp {
                snap: g,
                w,
                ranges: (!ranges.is_empty()).then_some(&ranges),
                sorted_probe: self.sorted_probe,
                interrupt: self.interrupt,
                budget: self.budget,
            };
            let wide = std::env::var_os("TRIPLELIGHT_FORCE_WIDE").is_some()
                && std::env::var_os("TRIPLELIGHT_DISABLE_WIDE").is_none();
            let star = wide
                .then(|| table_plan::recognize_star(&pats, unbound_start))
                .flatten();
            let tables = star
                .as_ref()
                .and_then(|s| g.wide_tables().map(|c| c.tables_with(&s.predicates)));
            if let (Some(star), Some(tables), Some(catalog)) = (star, tables, g.wide_tables())
                && !tables.is_empty()
            {
                (
                    table_plan::evaluate(
                        self,
                        g,
                        catalog,
                        &star,
                        &tables,
                        &pats,
                        flat,
                        filters,
                        &ranges,
                        tr.as_mut(),
                    ),
                    "wide-forced",
                )
            } else if self.wcoj
                && unbound_start
                && pats.len() >= 3
                && pats.iter().all(pattern::no_repeated_var)
                && pattern::cyclic(&pats)
            {
                let given = self.variable_order(&pats);
                let t = tr.as_mut().map(|tr| {
                    tr.note(
                        "var_order",
                        given.as_deref().map_or_else(
                            || "default".to_string(),
                            |o| plan_trace::var_names(o, &self.vars),
                        ),
                    );
                    std::time::Instant::now()
                });
                let mut out = bgp.leapfrog(&pats, &flat, given);
                let joined = out.len() / w;
                if !filters.is_empty() {
                    let all: HashSet<usize> = (0..w).collect();
                    self.apply_filters(
                        &mut out,
                        w,
                        filters,
                        &all,
                        &mut vec![false; filters.len()],
                        &mut numeric_filter_memo,
                    )
                }
                if let (Some(tr), Some(t)) = (tr.as_mut(), t) {
                    tr.step(None, "leapfrog", 1, joined, out.len() / w, t)
                }
                (out, "leapfrog")
            } else {
                let mut applied = vec![false; filters.len()];
                let mut post = |rows: &mut Vec<TermId>, bound: &HashSet<usize>| {
                    self.apply_filters(
                        rows,
                        w,
                        filters,
                        bound,
                        &mut applied,
                        &mut numeric_filter_memo,
                    )
                };
                match self.planned_order_est(&pats, &rows, &ranges, tr.is_some()) {
                    Some((order, est)) => {
                        if let Some(tr) = tr.as_mut() {
                            tr.planned(&order, est)
                        }
                        (
                            bgp.eval_order(&pats, &order, flat, &mut post, tr.as_mut()),
                            "planner",
                        )
                    }
                    None => (
                        bgp.eval_greedy(&pats, flat, &mut post, tr.as_mut()),
                        "greedy",
                    ),
                }
            }
        });
        if let Some(tr) = tr {
            tr.finish(strategy, out.len() / w)
        }
        out
    }
}
