//! Range-filter pushdown (`Options::range_pushdown`): a BGP pattern `?s <p> ?d` whose object is constrained by FILTER
//! conjuncts `?d >= c`, `?d > c`, `?d <= c`, `?d < c` (or mirrored) with constant `xsd:date` literals scans only the
//! objects of `<p>` whose IDs can satisfy them, instead of every object. The conjuncts stay pushed filters of the
//! BGP (`apply_filters`), so the ID interval only has to be a superset of the matching terms, never exact.
//!
//! # Why an ID interval is a superset
//!
//! 1. Within a kind, a unified disk dictionary numbers terms in the order of their N-Triples text
//!    (`Term::to_string()`, the text `find_term` and `check_text_order` compare). `Dictionary::literal_text_bound`
//!    answers the smallest stored literal ID whose text is `>= t` (or `> t`) only for such stores; every other store
//!    answers `None` and the pushdown is off.
//! 2. Only an `xsd:date` literal can satisfy a comparison with an `xsd:date` constant: `expr::compare` requires the
//!    same datatype (IRIs, blank nodes, triple terms, other datatypes, language and directional strings are type
//!    errors, i.e. false), and it compares two dates by their lexical forms.
//! 3. Suppose every `xsd:date` literal among the stored-literal objects of `<p>` is canonical: exactly
//!    `^\d{4}-\d{2}-\d{2}$` (four-digit year, no timezone). The constants are required to be canonical too. Such a
//!    literal's text is `"` + the 10-byte form + `"^^<http://www.w3.org/2001/XMLSchema#date>`: no character of the
//!    form is escaped, every text has the same prefix and suffix around a same-length middle, so byte order of two
//!    texts is the byte order of their forms, which is the order `expr::compare` uses (and, for this fixed-width
//!    form, the value order). So `v >= c` iff `text(v) >= text(c)` iff `id(v) >= literal_text_bound(text(c), false)`,
//!    and likewise `v > c` with `strict`, `v < c` iff `id(v) < literal_text_bound(text(c), false)` and `v <= c` iff
//!    `id(v) < literal_text_bound(text(c), true)`. Other literals whose text falls inside the interval (an
//!    `xsd:string` "2007-10-15", a `dateTime` "2007-10-15T..." whose text sorts between two dates) are scanned and
//!    then removed by the residual filter.
//!    Non-canonical forms break step 3. `"2007-10-10 "` (trailing space) satisfies `?d >= "2007-10-10"` (it is the
//!    longer string) but its text sorts first (space < `"`); `"2007-10-1\n"` satisfies `?d < "2007-10-11"` but its
//!    text sorts after (the escape `\` > `1`); negative, five-digit-year or timezoned forms do not sort like their
//!    values. If any `xsd:date` object of `<p>` is not canonical, the pattern keeps its full scan.
//! 4. The condition is checked per predicate by enumerating the distinct stored-literal objects of `<p>` with index
//!    seeks and decoding them once (at most `CHECK_LIMIT`; more, and the pushdown is off). The result is kept in the
//!    snapshot's `object_memo` (disk bases, and overlays of their own contents), so a store pays for it once per
//!    predicate, not per query. That needs no statistics or format change and holds for exactly the rows the
//!    pattern can produce.
//! 5. IDs that are not base dictionary literals are not in text order: overlay delta terms (`id_tag::DELTA`),
//!    scratch and inline IDs. All of them sit above every literal and triple-term tag, so the pattern also scans every
//!    object at or above `id_tag::DELTA` unrestricted; the residual filter decides those. Triples inserted with an
//!    already stored literal object carry a base ID and are found by the merged overlay scan inside the interval.
//!
//! A range is pushed only if it holds at most `MAX_FRACTION` of the predicate's triples (exact per-object counts
//! against `count(POS, [p])`); a denser one leaves the BGP's plan exactly as without the pushdown. Each decision is
//! logged on the server's `phase_trace` line (`range_pushdown=on:0.031,...`) under `TRIPLELIGHT_TRACE_PHASES`.
use super::bgp::Slot;
use super::{storage_failure, Ctx, UNBOUND};
use crate::model::{id_tag, Perm, TermId};
use crate::sparql::Expression;
use crate::storage::Snapshot;
use triplelight_rdf::vocab::xsd;
use triplelight_rdf::{Literal, TermRef};
use std::collections::HashMap;
use std::sync::Arc;

/// Distinct stored-literal objects of one predicate decoded for the canonical-form check; above this the pushdown is
/// off for that predicate.
const CHECK_LIMIT: usize = 1 << 16;
const FIRST_LITERAL: TermId = TermId(id_tag::LITERAL << 60);
const END_LITERAL: TermId = TermId((id_tag::LITERAL + 1) << 60);
/// The largest share of a predicate's triples a pushed range may hold (the selectivity gate in `object_range`).
const MAX_FRACTION: f64 = 0.15;
/// IDs from here up (delta, scratch, inline) are not in base dictionary order.
const FIRST_UNORDERED: TermId = TermId(id_tag::DELTA << 60);

/// The objects a ranged pattern scans (ascending: the literal ID interval, then every unordered ID) and the number of
/// its triples with those objects.
pub(super) struct ObjRange {
    pub objects: Vec<TermId>,
    pub rows: u64,
}

/// Ranged patterns of one BGP, by (predicate, object variable column).
pub(super) type Ranges = HashMap<(TermId, usize), ObjRange>;

/// The datatypes whose canonical forms are fixed-width and sort like their values, with that form's test. Adding
/// xsd:dateTime needs a fixed-width form (no fractional seconds, one timezone convention) and this table entry.
fn canonical(datatype: &str, value: &str) -> bool {
    datatype == xsd::DATE.as_str() && canonical_date(value)
}

/// `^\d{4}-\d{2}-\d{2}$`.
fn canonical_date(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() == 10 && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

/// The constant's datatype if `l` is a supported literal in canonical form.
fn ordered_constant(l: &Literal) -> Option<&'static str> {
    let dt = xsd::DATE.as_str();
    (l.datatype().as_str() == dt && canonical(dt, l.value())).then_some(dt)
}

/// One side of an interval, from one FILTER conjunct.
#[derive(Clone, Copy)]
enum Side {
    /// `?v > c` (strict) or `?v >= c`.
    Lower { strict: bool },
    /// `?v <= c` (inclusive) or `?v < c`.
    Upper { inclusive: bool },
}

struct Bound {
    side: Side,
    datatype: &'static str,
    text: String,
}

impl<S: Snapshot + ?Sized> Ctx<'_, S> {
    /// `(column, bound)` if `e` is a comparison of a variable with an order-preserving constant. Negations,
    /// disjunctions and anything else are not bounds (they stay ordinary filters).
    fn range_bound(&self, e: &Expression) -> Option<(usize, Bound)> {
        let (side, v, l) = match e {
            Expression::Greater(a, b) | Expression::GreaterOrEqual(a, b) | Expression::Less(a, b) | Expression::LessOrEqual(a, b) => {
                let (var_left, v, l) = match (a.as_ref(), b.as_ref()) {
                    (Expression::Variable(v), Expression::Literal(l)) => (true, v, l),
                    (Expression::Literal(l), Expression::Variable(v)) => (false, v, l),
                    _ => return None,
                };
                // Normalize to `?v op c`: a mirrored comparison (`c op ?v`) flips the direction.
                let (greater, or_equal) = match e {
                    Expression::Greater(..) => (true, false),
                    Expression::GreaterOrEqual(..) => (true, true),
                    Expression::Less(..) => (false, false),
                    _ => (false, true),
                };
                let side = if greater == var_left { Side::Lower { strict: !or_equal } } else { Side::Upper { inclusive: or_equal } };
                (side, v, l)
            }
            _ => return None,
        };
        let datatype = ordered_constant(l)?;
        let col = *self.vars.get(v.as_str())?;
        Some((col, Bound { side, datatype, text: triplelight_rdf::Term::from(l.clone()).to_string() }))
    }

    /// The ranged patterns of a BGP: `?s <p> ?d` where some pushed conjuncts bound `?d`, the
    /// store orders literal IDs by text, and `<p>`'s stored `xsd:date` objects are canonical. Empty when the switch
    /// is off, inside GRAPH / FROM (the active graph is not `snap`), or nothing applies.
    /// Patterns whose subject or object `first` (the first input row) binds are skipped: they are read through the
    /// bound value, and computing their range would only cost (e.g. once per row of an EXISTS).
    pub(super) fn object_ranges(&self, pats: &[[Slot; 3]], filters: &[(&Expression, Vec<usize>)], first: &[TermId]) -> Ranges {
        let mut out = Ranges::new();
        if !self.range_pushdown || filters.is_empty() || self.data.get().is_some() { return out }
        let mut bounds: HashMap<usize, Vec<Bound>> = HashMap::new();
        for (e, _) in filters {
            if let Some((col, b)) = self.range_bound(e) { bounds.entry(col).or_default().push(b) }
        }
        if bounds.is_empty() { return out }
        for pat in pats {
            // A constant subject is read through SPO as usual (one subject's objects need no range).
            let [Slot::Var(s), Slot::Const(Some(p)), Slot::Var(o)] = *pat else { continue };
            if s == o || first[s] != UNBOUND || first[o] != UNBOUND || out.contains_key(&(p, o)) { continue }
            let Some(bs) = bounds.get(&o) else { continue };
            if let Some(r) = self.object_range(p, bs) { out.insert((p, o), r); }
        }
        out
    }

    fn object_range(&self, p: TermId, bounds: &[Bound]) -> Option<ObjRange> {
        let datatype = bounds[0].datatype;
        // Bounds of different datatypes cannot all hold; leave such a conjunction to the filter.
        if bounds.iter().any(|b| b.datatype != datatype) { return None }
        let literals = self.ordered_literal_objects(p, datatype)?;
        let (mut lo, mut hi) = (FIRST_LITERAL, END_LITERAL);
        for b in bounds {
            let strict = match b.side { Side::Lower { strict } => strict, Side::Upper { inclusive } => inclusive };
            let id = self.snap.literal_text_bound(&b.text, strict).unwrap_or_else(|e| storage_failure(e))?;
            match b.side { Side::Lower { .. } => lo = lo.max(id), Side::Upper { .. } => hi = hi.min(id) }
        }
        let mut objects: Vec<TermId> = if lo < hi {
            let a = literals.partition_point(|&x| x < lo);
            let b = literals.partition_point(|&x| x < hi);
            literals[a..b].to_vec()
        } else { Vec::new() };
        self.objects_until(p, FIRST_UNORDERED, None, usize::MAX, &mut objects);
        // Selectivity gate: a range holding more than `MAX_FRACTION` of the predicate's triples is not pushed, so the
        // pattern keeps exactly the unranged plan (reading most of a predicate object by object, then joining in
        // object order, loses to its subject-ordered scan). Exact counts, one per object, stopping past the limit.
        let total = self.snap.count(Perm::Pos, &[p]).unwrap_or_else(|e| storage_failure(e));
        let limit = (total as f64 * MAX_FRACTION) as u64;
        let mut rows = 0u64;
        for &o in &objects {
            rows += self.snap.count(Perm::Pos, &[p, o]).unwrap_or_else(|e| storage_failure(e));
            if rows > limit { break }
        }
        let frac = if total == 0 { 0.0 } else { rows as f64 / total as f64 };
        let pushed = rows <= limit;
        super::phase::range_decision(pushed, frac);
        pushed.then_some(ObjRange { objects, rows })
    }

    /// Appends the distinct objects of `p` in `[from, to)` (`to = None`: no upper end), ascending, found by index
    /// seeks. Returns false, having appended `limit` of them, if there are more than `limit`.
    fn objects_until(&self, p: TermId, from: TermId, to: Option<TermId>, limit: usize, out: &mut Vec<TermId>) -> bool {
        let mut x = from;
        let start = out.len();
        while let Some(o) = self.snap.seek(Perm::Pos, &[p], x).unwrap_or_else(|e| storage_failure(e)) {
            if to.is_some_and(|t| o >= t) { break }
            if out.len() - start == limit { return false }
            out.push(o);
            if o.0 == u64::MAX { break }
            x = TermId(o.0 + 1);
        }
        true
    }

    /// The stored-literal objects of `p`, ascending, if every one of `datatype` is in canonical form (condition 3 of
    /// the module documentation); `None` otherwise or above `CHECK_LIMIT` objects. Cached in the snapshot's
    /// `object_memo` (immutable views only), else per query.
    fn ordered_literal_objects(&self, p: TermId, datatype: &'static str) -> Option<Arc<[TermId]>> {
        let key = (p, datatype);
        if let Some(r) = self.range_checks.borrow().get(&key) { return r.clone() }
        let memo = self.snap.object_memo();
        let cached = memo.and_then(|m| { let memo = m.0.lock().ok()?; memo.get(&key).cloned() });
        let r = match cached { Some(r) => r, None => self.check_literal_objects(p, datatype) };
        if let Some(m) = memo && let Ok(mut m) = m.0.lock() { m.insert(key, r.clone()); }
        self.range_checks.borrow_mut().insert(key, r.clone());
        r
    }

    fn check_literal_objects(&self, p: TermId, datatype: &'static str) -> Option<Arc<[TermId]>> {
        let mut ids = Vec::new();
        let mut ok = self.objects_until(p, FIRST_LITERAL, Some(END_LITERAL), CHECK_LIMIT, &mut ids);
        if ok {
            self.snap.with_terms(&ids, &mut |_, t| {
                if let TermRef::Literal(l) = t && l.datatype().as_str() == datatype && !canonical(datatype, l.value()) { ok = false }
            }).unwrap_or_else(|e| storage_failure(e));
        }
        ok.then(|| ids.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_dates_only() {
        for ok in ["2007-10-10", "0000-01-01", "9999-12-31"] { assert!(canonical_date(ok), "{ok}") }
        for bad in ["2007-10-10Z", " 2007-10-10", "2007-10-10 ", "-2007-10-10", "12007-10-10", "2007-1-10", "2007/10/10", "2007-10-10+01:00", ""] {
            assert!(!canonical_date(bad), "{bad:?}")
        }
    }

    /// The ordering argument of the module documentation, checked on the texts themselves: for canonical dates,
    /// N-Triples text order is lexical-form order.
    #[test]
    fn canonical_date_texts_sort_like_their_forms() {
        let forms = ["0999-12-31", "2007-01-01", "2007-10-09", "2007-10-10", "2007-10-11", "2007-11-07", "2008-01-01"];
        let text = |f: &str| triplelight_rdf::Term::from(Literal::new_typed_literal(f, xsd::DATE)).to_string();
        for a in forms { for b in forms { assert_eq!(a.cmp(b), text(a).cmp(&text(b)), "{a} {b}") } }
    }
}
