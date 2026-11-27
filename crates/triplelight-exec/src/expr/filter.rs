//! Pushed FILTER conjuncts over a BGP's flat rows (`eval_bgp_flat`): each one runs as soon as its variables are
//! bound, through an ID comparison where the conjunct allows one and the memoized evaluator otherwise.
use crate::{expr, id_hash, impure, phase, Ctx, Row, UNBOUND};
use crate::model::TermId;
use crate::sparql::Expression;
use crate::storage::Snapshot;
use std::collections::HashSet;
use triplelight_rdf::Term;

impl<S: Snapshot + ?Sized> Ctx<'_, S> {
    /// Applies each not-yet-applied filter whose variables are all bound, keeping rows whose filter is true. With
    /// `mark`, records those filters as applied; without it (one batch of a step's output, `Bgp::join_step`) they stay
    /// pending, so the step's other batches and its final call apply them too.
    pub(crate) fn apply_filters(&self, rows: &mut Vec<TermId>, w: usize, filters: &[(&Expression, Vec<usize>)], bound: &HashSet<usize>, applied: &mut [bool], mark: bool, numeric_memo: &mut id_hash::IdMap<Option<expr::Num>>) {
        phase::filter(|| for (k, (e, vs)) in filters.iter().enumerate() {
            if applied[k] || !vs.iter().all(|v| bound.contains(v)) { continue }
            if mark { applied[k] = true }
            if let Some(kept) = self.filter_text_literals(e, rows, w) {
                *rows = kept;
                continue;
            }
            let mut kept = Vec::with_capacity(rows.len());
            let mut row: Row = vec![UNBOUND; w];
            if let Some((col, c, eq)) = self.id_equality(e) {
                // `?x = <iri>` / `?x != <iri>` (also sameTerm) on stored terms is ID (in)equality: the dictionary maps
                // each IRI/blank node to one ID, and an IRI never equals a literal or another IRI. Scratch IDs
                // (computed terms) take the general path.
                let mut kept = Vec::with_capacity(rows.len());
                for r in rows.chunks_exact(w) {
                    let x = r[col];
                    let keep = if x == UNBOUND { false } else if x.0 >> 60 == 0xe {
                        row.copy_from_slice(r); self.ebv(e, &row)
                    } else { (Some(x) == c) == eq };
                    if keep { kept.extend_from_slice(r) }
                }
                *rows = kept;
                continue;
            }
            let fast = expr::NumFilter::compile(e, &self.vars);
            let mut val = |id: TermId| *numeric_memo.entry(id).or_insert_with(|| expr::term_num(&self.resolve(id)));
            // A filter over one variable (and no EXISTS) depends only on that column's term: memoize its result per
            // term ID, so e.g. a date range over many reviews decodes each distinct date once.
            let single = match vs.as_slice() { [c] if !impure(e) => Some(*c), _ => None };
            let mut by_id: id_hash::IdMap<bool> = id_hash::IdMap::default();
            for (n, r) in rows.chunks_exact(w).enumerate() {
                if n.is_multiple_of(4096) { self.check() }
                let keep = match fast.as_ref().map(|f| f.eval(r, &mut val)) {
                    Some(Ok(b)) => b == Some(true),
                    _ => match single {
                        Some(c) => *by_id.entry(r[c]).or_insert_with(|| { row.copy_from_slice(r); self.ebv(e, &row) }),
                        None => { row.copy_from_slice(r); self.ebv(e, &row) }
                    },
                };
                if keep { kept.extend_from_slice(r) }
            }
            *rows = kept;
        })
    }

    /// `(column, constant's stored ID, is_equality)` for `?x = <iri>`, `?x != <iri>`, `sameTerm(?x, <iri>)` and their
    /// mirrored forms, where the constant is an IRI (blank nodes cannot appear as query constants).
    fn id_equality(&self, e: &Expression) -> Option<(usize, Option<TermId>, bool)> {
        let (a, b, eq) = match e {
            Expression::Equal(a, b) | Expression::SameTerm(a, b) => (a, b, true),
            Expression::Not(inner) => match inner.as_ref() { Expression::Equal(a, b) => (a, b, false), _ => return None },
            _ => return None,
        };
        let (v, n) = match (a.as_ref(), b.as_ref()) {
            (Expression::Variable(v), Expression::NamedNode(n)) | (Expression::NamedNode(n), Expression::Variable(v)) => (v, n),
            _ => return None,
        };
        let col = *self.vars.get(v.as_str())?;
        Some((col, self.lookup(&Term::NamedNode(n.clone())), eq))
    }
}
