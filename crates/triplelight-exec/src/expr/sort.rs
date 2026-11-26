//! ORDER BY over materialized rows (`Ctx::sort_rows`), in memory or, under the memory budget, as an external merge
//! sort (#111). The in-memory sort holds one decoded term per row and key; when those keys do not fit what is free
//! (and the budget can spill), the rows are sorted in runs that fit, each written to a run file, and the runs are
//! merged. Ties keep input order in both: a run covers consecutive input rows and is sorted stably (`order::order`),
//! and the merge breaks ties on the input position, so the answer is the in-memory one, order included. A LIMIT
//! (top-k) never spills: it keeps the keys of the candidates only.
use super::order::order;
use super::order_cmp;
use crate::limits::spill;
use crate::model::TermId;
use crate::sparql::{Expression, OrderExpression};
use crate::storage::Snapshot;
use crate::{storage_failure, Ctx, Row, UNBOUND};
use std::cmp::Ordering;
use crate::id_hash::FastMap;
use triplelight_rdf::Term;

/// Each ORDER BY key: its expression, and the column holding it when it is a variable (or the alias computed below).
type KeyColumn<'e> = (&'e Expression, Option<usize>);

/// Bytes of one decoded sort key, as charged.
const KEY_BYTES: u64 = 48;
/// The most runs merged at once (one open file each).
const MAX_RUNS: usize = spill::MAX_PARTITIONS;

impl<S: Snapshot + ?Sized> Ctx<'_, S> {
    pub(crate) fn sort_rows(
        &self,
        expression: &[OrderExpression],
        limit: Option<usize>,
        rows: Vec<Row>,
        alias: Option<(&Expression, usize)>,
    ) -> Vec<Row> {
        let keys_by_column: Vec<KeyColumn<'_>> = expression.iter().map(|key| {
            let e = match key { OrderExpression::Asc(e) | OrderExpression::Desc(e) => e };
            let column = match e {
                Expression::Variable(v) => self.vars.get(v.as_str()).copied(),
                e => alias.filter(|(bound, _)| *bound == e).map(|(_, col)| col),
            };
            (e, column)
        }).collect();
        let desc: Vec<bool> = expression.iter().map(|e| matches!(e, OrderExpression::Desc(_))).collect();
        let keys = rows.len() as u64 * desc.len() as u64 * KEY_BYTES;
        if limit.is_none() && rows.len() > 1 && self.budget.is_some_and(|b| b.spill_dir_for(rows.len(), keys.saturating_mul(2)).is_some()) {
            return self.sort_spilled(&keys_by_column, &desc, rows);
        }
        // Column keys decode once per distinct ID. Other expressions are evaluated per row.
        let mut memo: FastMap<TermId, Option<Term>> = FastMap::default();
        let mut computed = 0usize;
        let mut key_of = |r: &Row, (e, column): KeyColumn<'_>| -> Option<Term> {
            computed += 1;
            if computed.is_multiple_of(4096) { self.check() }
            match column {
                Some(i) if r[i] == UNBOUND => None,
                Some(i) => memo.entry(r[i]).or_insert_with(|| Some(self.resolve(r[i]))).clone(),
                None => self.value(e, r),
            }
        };
        // Sort keys: one decoded term (about 48 bytes) per row and key, at most.
        let _keys = self.reserve(keys);
        let idx = order(rows.len(), &desc, limit, |i, k| key_of(&rows[i], keys_by_column[k]));
        let mut rows: Vec<Option<Row>> = rows.into_iter().map(Some).collect();
        idx.into_iter().map(|i| rows[i].take().unwrap()).collect()
    }

    /// One row's key `k`, with no memo (the spilled sort keeps its key memory bounded).
    fn sort_key(&self, r: &Row, (e, column): KeyColumn<'_>) -> Option<Term> {
        match column {
            Some(i) if r[i] == UNBOUND => None,
            Some(i) => Some(self.resolve(r[i])),
            None => self.value(e, r),
        }
    }

    fn sort_spilled(&self, keys: &[KeyColumn<'_>], desc: &[bool], rows: Vec<Row>) -> Vec<Row> {
        let budget = self.budget.expect("spill needs a budget");
        let dir = budget.scratch_dir().expect("spill needs a scratch directory");
        let n = rows.len();
        let per_row = desc.len() as u64 * KEY_BYTES + self.row_bytes();
        // A run's keys and rows take a quarter of what is free; at most MAX_RUNS runs, so a huge input gets longer runs.
        let mut run_rows = ((budget.available() / 4) / per_row.max(1)).max(1024) as usize;
        run_rows = run_rows.max(n.div_ceil(MAX_RUNS));
        // A forced spill (tests) gets several runs even when everything fits.
        if budget.forces_spill(n) { run_rows = run_rows.min(n.div_ceil(4)).max(1) }
        let files = n.div_ceil(run_rows);
        // All run files are written at once: their buffers take at most a quarter of what is free.
        let buffer = (budget.available() / (4 * files as u64)).clamp(4 << 10, 64 << 10) as usize;
        let runs = {
            let _buffers = self.reserve((files * buffer) as u64);
            // One run's keys are held while it is sorted; runs are sorted one at a time, as the writer reaches them.
            let _run = self.reserve(run_rows as u64 * desc.len() as u64 * KEY_BYTES);
            let records = rows.chunks(run_rows).enumerate().flat_map(|(c, chunk)| {
                let idx = order(chunk.len(), desc, None, |i, k| self.sort_key(&chunk[i], keys[k]));
                let start = (c * run_rows) as u64;
                idx.into_iter().map(move |i| (c, start + i as u64, chunk[i].as_slice()))
            });
            spill::Spill::write(dir, self.vars.len(), (files, buffer), records, || self.check()).unwrap_or_else(|e| storage_failure(e))
        };
        drop(rows);
        self.merge_runs(&runs, keys, desc, n)
    }

    /// Merges sorted runs: the smallest head by key, then by input position, repeatedly.
    fn merge_runs(&self, runs: &spill::Spill, keys: &[KeyColumn<'_>], desc: &[bool], n: usize) -> Vec<Row> {
        let k = runs.files();
        let _heads = self.reserve(k as u64 * (desc.len() as u64 * KEY_BYTES + self.row_bytes() + (16 << 10)));
        let mut readers: Vec<_> = (0..k).map(|r| runs.read(r, 16 << 10).unwrap_or_else(|e| storage_failure(e))).collect();
        let head = |reader: &mut dyn Iterator<Item = std::io::Result<(u64, Row)>>| -> Option<(Vec<Option<Term>>, u64, Row)> {
            let (seq, row) = reader.next()?.unwrap_or_else(|e| storage_failure(e));
            Some((keys.iter().map(|&kc| self.sort_key(&row, kc)).collect(), seq, row))
        };
        let mut heads: Vec<Option<(Vec<Option<Term>>, u64, Row)>> = readers.iter_mut().map(|r| head(r)).collect();
        let before = |a: &(Vec<Option<Term>>, u64, Row), b: &(Vec<Option<Term>>, u64, Row)| -> bool {
            for (i, &d) in desc.iter().enumerate() {
                let c = order_cmp(a.0[i].as_ref(), b.0[i].as_ref());
                let c = if d { c.reverse() } else { c };
                if c != Ordering::Equal { return c == Ordering::Less }
            }
            a.1 < b.1
        };
        let mut out = Vec::with_capacity(n);
        loop {
            if out.len().is_multiple_of(4096) { self.check() }
            let mut best: Option<usize> = None;
            for (r, h) in heads.iter().enumerate() {
                let Some(h) = h else { continue };
                if best.is_none_or(|b| before(h, heads[b].as_ref().unwrap())) { best = Some(r) }
            }
            let Some(b) = best else { break };
            let next = head(&mut readers[b]);
            let (_, _, row) = std::mem::replace(&mut heads[b], next).unwrap();
            out.push(row);
        }
        out
    }
}
