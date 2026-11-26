//! Flat BGP execution and optional wide-table access.
use super::{Bgp, table_plan};
use crate::*;
use std::time::Instant;
use triplelight_core::wide::{TableCatalog, TableId};

impl<'a, S: Snapshot + ?Sized> Ctx<'a, S> {
    /// The first, explicitly forced physical path. It unions every matching table
    /// with the ordinary BGP restricted to subjects absent from those tables.
    /// The residual's first join step still scans an index; after that step its
    /// subject filter prevents another expensive join on already covered rows.
    fn evaluate_wide<G: Snapshot + ?Sized>(
        &self,
        snap: &G,
        catalog: &dyn TableCatalog,
        star: &table_plan::Star,
        tables: &[TableId],
        patterns: &[[Slot; 3]],
        input: Vec<TermId>,
        filters: &[(&Expression, Vec<usize>)],
        ranges: &range::Ranges,
        whole_bgp: bool,
        mut trace: Option<&mut plan_trace::Trace>,
    ) -> Vec<TermId> {
        let width = self.vars.len();
        let bound: HashSet<usize> = std::iter::once(star.subject_slot)
            .chain(star.columns.iter().filter_map(|(_, object)| match object {
                Slot::Var(slot) => Some(*slot),
                Slot::Const(_) => None,
            }))
            .collect();
        let predicates: Vec<TermId> = star.columns.iter().map(|(p, _)| *p).collect();
        let table_ranges = self.table_intervals(&star.columns, filters);
        let mut numeric_memo: crate::id_hash::IdMap<Option<expr::Num>> = crate::id_hash::IdMap::default();
        let mut out = Vec::new();
        let started = trace.as_ref().map(|_| Instant::now());
        let mut scanned = 0u64;
        let mut emitted = 0u64;
        let mut bytes = 0u64;
        for &table in tables {
            let mut scan = catalog.scan(table, &predicates, &table_ranges);
            while let Some(batch) = scan.next_batch() {
                self.check();
                let batch = batch.unwrap_or_else(|error| storage_failure(error));
                self.fits(
                    (out.len().saturating_add(batch.len().saturating_mul(width)) as u64)
                        .saturating_mul(8),
                );
                let mut flat = Vec::with_capacity(batch.len().saturating_mul(width));
                for row in 0..batch.len() {
                    let base = flat.len();
                    flat.resize(base + width, UNBOUND);
                    flat[base + star.subject_slot] = batch.subjects[row];
                    for (column, &(_, object)) in star.columns.iter().enumerate() {
                        if let Slot::Var(slot) = object {
                            flat[base + slot] = batch.values[column][row];
                        }
                    }
                }
                self.fits((out.len().saturating_add(flat.len()) as u64).saturating_mul(8));
                self.apply_filters(
                    &mut flat,
                    width,
                    filters,
                    &bound,
                    &mut vec![false; filters.len()],
                    &mut numeric_memo,
                );
                out.extend(flat);
            }
            let counters = scan.counters();
            scanned += counters.subjects_scanned;
            emitted += counters.rows_emitted;
            bytes += counters.bytes_read;
        }
        if let (Some(trace), Some(started)) = (trace.as_deref_mut(), started) {
            trace.note("tables", tables.len());
            trace.note("star_patterns", star.pattern_indices.len());
            trace.note("table_ranges", table_ranges.iter().filter(|r| r.is_some()).count());
            trace.note("table_subjects", scanned);
            trace.note("table_bytes", bytes);
            trace.step(
                None,
                "wide",
                1,
                emitted as usize,
                out.len() / width,
                started,
            );
        }

        // Every selected type-group table contains every predicate observed for
        // its subjects, and `tables` includes every table with the full star
        // schema. If every subject with one star predicate is in some table,
        // every matching subject is in a selected table; no residual can match.
        if predicates.iter().any(|&p| catalog.covers_all_subjects(p)) {
            return out;
        }

        // The table partition is subject-disjoint. Keep the index BGP's ordinary
        // access order, but prune covered subjects as soon as the first pattern
        // binds them so the following patterns do not redo their joins.
        let bgp = Bgp {
            snap,
            w: width,
            ranges: (!ranges.is_empty()).then_some(ranges),
            sorted_probe: self.sorted_probe,
            interrupt: self.interrupt,
            budget: self.budget,
        };
        let mut applied = vec![false; filters.len()];
        let mut pruned = false;
        let mut post = |rows: &mut Vec<TermId>, available: &HashSet<usize>| {
            if !pruned && available.contains(&star.subject_slot) {
                let mut kept = Vec::with_capacity(rows.len());
                for row in rows.chunks_exact(width) {
                    if !tables
                        .iter()
                        .any(|&table| catalog.member(table, row[star.subject_slot]))
                    {
                        kept.extend_from_slice(row);
                    }
                }
                *rows = kept;
                pruned = true;
            }
            self.apply_filters(
                rows,
                width,
                filters,
                available,
                &mut applied,
                &mut numeric_memo,
            );
        };
        let initial = vec![vec![UNBOUND; width]];
        // A partial star is renumbered locally. Its pattern IDs do not match
        // the full BGP trace, so omit per-pattern residual trace in that case.
        let residual = match self.planned_order_est(patterns, &initial, ranges, whole_bgp && trace.is_some()) {
            Some((order, estimates)) => {
                if let Some(trace) = trace.as_deref_mut().filter(|_| whole_bgp) {
                    trace.planned(&order, estimates);
                }
                bgp.eval_order(patterns, &order, input, &mut post, trace.as_deref_mut().filter(|_| whole_bgp))
            }
            None => bgp.eval_greedy(patterns, input, &mut post, trace.filter(|_| whole_bgp)),
        };
        self.fits((out.len().saturating_add(residual.len()) as u64).saturating_mul(8));
        out.extend(residual);
        out
    }

    /// `eval_bgp_slots` for a query with at least one variable, as one flat buffer of rows (`vars.len()` IDs each),
    /// so a consumer that reads the rows once (`agg::flat`) skips building a `Row` per solution.
    pub(crate) fn eval_bgp_flat(
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
        let mut numeric_filter_memo: crate::id_hash::IdMap<Option<expr::Num>> = crate::id_hash::IdMap::default();
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
            let candidate = self.force_wide_tables.then(|| g.wide_tables()).flatten()
                .and_then(|catalog| {
                    table_plan::recognize_stars(&pats, unbound_start).into_iter().find_map(|star| {
                        let tables = catalog.tables_with(&star.predicates);
                        (!tables.is_empty()).then_some((star, tables, catalog))
                    })
                });
            if let Some((star, tables, catalog)) = candidate {
                let star_pats: Vec<_> = star.pattern_indices.iter().map(|&i| pats[i]).collect();
                let mut out = self.evaluate_wide(
                    g, catalog, &star, &tables, &star_pats, flat, filters, &ranges,
                    star_pats.len() == pats.len(),
                    tr.as_mut(),
                );
                if star_pats.len() != pats.len() && !out.is_empty() {
                    let remaining: Vec<_> = pats.iter().enumerate()
                        .filter_map(|(i, pat)| (!star.pattern_indices.contains(&i)).then_some(*pat))
                        .collect();
                    let bound: HashSet<usize> = std::iter::once(star.subject_slot)
                        .chain(star.columns.iter().filter_map(|(_, object)| match object {
                            Slot::Var(slot) => Some(*slot),
                            Slot::Const(_) => None,
                        }))
                        .collect();
                    let mut applied: Vec<bool> = filters.iter()
                        .map(|(_, vs)| vs.iter().all(|slot| bound.contains(slot)))
                        .collect();
                    let mut post = |rows: &mut Vec<TermId>, available: &HashSet<usize>| {
                        self.apply_filters(rows, w, filters, available, &mut applied, &mut numeric_filter_memo)
                    };
                    // The remaining patterns extend the table-produced rows
                    // through the ordinary indexes, preserving their joins.
                    out = bgp.eval_greedy(&remaining, out, &mut post, None);
                }
                (out, "wide-forced")
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
