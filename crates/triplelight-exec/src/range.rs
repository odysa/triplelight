//! Range-filter pushdown (`Options::range_pushdown`): a BGP pattern `?s <p> ?d` whose object is constrained by FILTER
//! conjuncts `?d >= c`, `?d > c`, `?d <= c`, `?d < c` (or mirrored) with canonical `xsd:date` or
//! integer/decimal constants scans only the
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
//! Numeric values have a separate proof. On first use per predicate and snapshot, at most `CHECK_LIMIT` distinct
//! stored literal objects are decoded in ID order. The executor's own numeric comparison must show that every
//! parseable numeric object is an integer or decimal and that values are nondecreasing. The verified numeric IDs
//! are cached; each later bound uses binary search. This refuses float/double predicates and nonmonotone integer
//! spellings (e.g. "9", "10"). Other datatypes inside a table interval are false positives removed by the residual
//! FILTER. A numeric bound never assumes that N-Triples lexical order is numeric value order.
//! The ordinary triple-index path requires `Options::numeric_range_pushdown` for numeric bounds; it is off by
//! default. Explicitly selected wide tables may still use numeric intervals when `range_pushdown` is on.
//!
//! A range is pushed only if it holds at most `MAX_FRACTION` of the predicate's triples (exact per-object counts
//! against `count(POS, [p])`); a denser one leaves the BGP's plan exactly as without the pushdown. Each decision is
//! logged on the server's `phase_trace` line (`range_pushdown=on:0.031,...`) under `TRIPLELIGHT_TRACE_PHASES`.
//! A wide-table scan instead applies the same canonical-literal bounds before row expansion, even for dense
//! ranges. It uses a single ID interval only when the predicate has no unordered overlay objects.
//!
//! A pushed range changes the join order and the order of the BGP's rows, never their multiset. It is therefore
//! applied only to BGPs whose solution order cannot reach the answers (`reorderable_bgps`, computed once per query):
//! not under SUM, AVG, MIN, MAX, SAMPLE, GROUP_CONCAT or a custom aggregate, nor under a LIMIT/OFFSET unless the
//! ORDER BY it slices is provably tie-free (`tie_free`).
use super::pattern::Slot;
use super::{storage_failure, Ctx, UNBOUND};
use super::expr;
use crate::model::{id_tag, Perm, TermId};
use crate::sparql::{AggregateExpression, AggregateFunction, Expression, GraphPattern};
use crate::storage::Snapshot;
use triplelight_rdf::vocab::xsd;
use triplelight_rdf::{Literal, NamedNode, TermRef};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use rust_decimal::Decimal;

/// Distinct stored-literal objects of one predicate decoded for the canonical-form check; above this the pushdown is
/// off for that predicate.
const CHECK_LIMIT: usize = 1 << 16;
const FIRST_LITERAL: TermId = TermId(id_tag::LITERAL << 60);
const END_LITERAL: TermId = TermId((id_tag::LITERAL + 1) << 60);
/// The largest share of a predicate's triples a pushed range may hold (the selectivity gate in `object_range`).
const MAX_FRACTION: f64 = 0.15;
/// IDs from here up (delta, scratch, inline) are not in base dictionary order.
const FIRST_UNORDERED: TermId = TermId(id_tag::DELTA << 60);
const NUMERIC_MARKER: &str = "ordered integer/decimal objects";

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
fn ordered_constant(l: &Literal, allow_numeric: bool) -> Option<&'static str> {
    let dt = xsd::DATE.as_str();
    if l.datatype().as_str() == dt && canonical(dt, l.value()) { return Some(dt) }
    allow_numeric.then(|| expr::range_decimal(l.as_ref())).flatten().map(|_| NUMERIC_MARKER)
}

/// Adds to `out` the BGPs of `p` (by the address of their pattern list) whose solution order cannot change the
/// query's answers, so a range pushdown, which changes join order and row order, may apply to them. `free` says
/// whether the solutions of `p` itself may be reordered (true at the top: SPARQL leaves solution order unspecified).
///
/// Order matters below a LIMIT/OFFSET without ORDER BY (which rows are selected) and below a GROUP with any aggregate
/// but COUNT: SUM and AVG fold left to right (integer overflow is an error, floats round by order), MIN and MAX keep
/// the first or last of numerically equal terms of different types, SAMPLE and GROUP_CONCAT take input order, and
/// a custom aggregate is unknown. A Group's own output order follows its input, so its input is free only if the
/// group's output is. Everything else passes `free` down; EXISTS patterns and anything this walk does not reach
/// (rewritten patterns) are never marked, so they keep the unranged plan.
pub(super) fn reorderable_bgps(p: &GraphPattern, free: bool, nodes: &dyn Fn(&NamedNode) -> bool, out: &mut HashSet<usize>) {
    match p {
        GraphPattern::Bgp { patterns } => { if free && !patterns.is_empty() { out.insert(patterns.as_ptr() as usize); } }
        GraphPattern::Path { .. } | GraphPattern::Values { .. } => {}
        GraphPattern::Join { left, right } | GraphPattern::Union { left, right } | GraphPattern::Minus { left, right }
        | GraphPattern::LeftJoin { left, right, .. } => { reorderable_bgps(left, free, nodes, out); reorderable_bgps(right, free, nodes, out) }
        GraphPattern::Filter { inner, .. } | GraphPattern::Graph { inner, .. } | GraphPattern::Extend { inner, .. }
        | GraphPattern::OrderBy { inner, .. } | GraphPattern::Project { inner, .. } | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner } | GraphPattern::Service { inner, .. } => reorderable_bgps(inner, free, nodes, out),
        GraphPattern::Slice { inner, .. } => {
            // Which rows a slice keeps depends on their order unless an ORDER BY under it (through projection and
            // duplicate removal) orders them without ties: the executor breaks ties by input order.
            reorderable_bgps(inner, free && tie_free(slice_input(inner), nodes), nodes, out)
        }
        GraphPattern::Group { inner, aggregates, .. } => {
            let counts_only = aggregates.iter().all(|(_, a)| matches!(a,
                AggregateExpression::CountSolutions { .. } | AggregateExpression::FunctionCall { name: AggregateFunction::Count, .. }));
            reorderable_bgps(inner, free && counts_only, nodes, out)
        }
    }
}

pub(super) fn slice_input(mut p: &GraphPattern) -> &GraphPattern {
    while let GraphPattern::Project { inner, .. } | GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner } = p {
        p = inner;
    }
    p
}

/// Whether every object of predicate `p` in `snap` (the merged view, so delta inserts count) is a stored IRI: POS
/// keeps `p`'s objects in ID order and the tag is the top four bits, so the first object must carry `id_tag::IRI` and
/// no object may be at or above the next tag (blank nodes, literals, triple terms, and delta, scratch and inline IDs
/// all are, so any of those refuses; tag 0 is never a stored ID but refuses too). Two seeks, memoized per snapshot
/// in its `object_memo`. An absent predicate binds nothing, so it qualifies.
pub(super) fn iri_objects<S: Snapshot + ?Sized>(snap: &S, p: &NamedNode) -> bool {
    let Ok(found) = snap.lookup(&p.clone().into()) else { return false };
    let Some(pid) = found else { return true };
    let key = (pid, "iri-only objects");
    let memo = snap.object_memo();
    if let Some(r) = memo.and_then(|m| { let memo = m.0.lock().ok()?; memo.get(&key).cloned() }) { return r.is_some() }
    let seek = |t: u64| snap.seek(Perm::Pos, &[pid], TermId(t));
    let ok = match (seek(1), seek((id_tag::IRI + 1) << 60)) {
        (Ok(first), Ok(None)) => first.is_none_or(|x| id_tag::of(x) == id_tag::IRI),
        _ => false,
    };
    if let Some(m) = memo && let Ok(mut m) = m.0.lock() { m.insert(key, ok.then(|| Arc::<[TermId]>::from(Vec::new()))); }
    ok
}

/// Whether `p` is an ORDER BY that provably has no ties: its input rows are unique on a set of variables (`unique_on`)
/// that are all plain keys of the ORDER BY, and each of those can only be bound to IRIs or blank nodes
/// (`node_only`; as an object, only IRIs: `iri_objects`). The one case accepted is rows from GROUP BY (or a join
/// of grouped subqueries), ordered by every grouping variable.
///
/// Why nodes only: `expr::order_cmp` orders two IRIs, and two blank nodes, by their text, which differs for distinct
/// stored terms, so distinct values never tie (and unbound only ties with unbound, i.e. the same group). Literals can
/// tie while distinct (integer 1 and decimal 1.0 compare equal but are different group keys).
fn tie_free(p: &GraphPattern, nodes: &dyn Fn(&NamedNode) -> bool) -> bool {
    let GraphPattern::OrderBy { inner, expression } = p else { return false };
    let keys: Vec<&str> = expression.iter().filter_map(|o| {
        let (crate::sparql::OrderExpression::Asc(e) | crate::sparql::OrderExpression::Desc(e)) = o;
        if let Expression::Variable(v) = e { Some(v.as_str()) } else { None }
    }).collect();
    unique_on(inner).is_some_and(|vs| vs.iter().all(|(v, grouped)| keys.contains(v) && node_only(v, grouped, nodes)))
}

/// Whether every way `p` can bind `v` binds an IRI or a blank node: subject or predicate, or the object of a constant
/// predicate whose objects in the default graph are all IRIs (`nodes`, i.e. `iri_objects`). A path endpoint,
/// VALUES, BIND, aggregate result, SERVICE, GRAPH or (RDF 1.2) triple-term pattern is not proven.
fn node_only(v: &str, p: &GraphPattern, nodes: &dyn Fn(&NamedNode) -> bool) -> bool {
    use crate::sparql::{NamedNodePattern, TermPattern};
    let is = |t: &TermPattern| matches!(t, TermPattern::Variable(x) if x.as_str() == v);
    match p {
        GraphPattern::Bgp { patterns } => patterns.iter().all(|t| {
            #[cfg(feature = "rdf-12")]
            if matches!(t.subject, TermPattern::Triple(_)) || matches!(t.object, TermPattern::Triple(_)) { return false }
            !is(&t.object) || matches!(&t.predicate, NamedNodePattern::NamedNode(n) if nodes(n))
        }),
        GraphPattern::Path { subject, object, .. } => !is(subject) && !is(object),
        GraphPattern::Values { variables, .. } => variables.iter().all(|x| x.as_str() != v),
        // Inside GRAPH the patterns read a named graph, where `nodes` (checked on the default graph) proves nothing.
        GraphPattern::Service { .. } | GraphPattern::Graph { .. } => false,
        GraphPattern::Extend { inner, variable, .. } => variable.as_str() != v && node_only(v, inner, nodes),
        GraphPattern::Group { inner, aggregates, .. } => aggregates.iter().all(|(x, _)| x.as_str() != v) && node_only(v, inner, nodes),
        GraphPattern::Join { left, right } | GraphPattern::Union { left, right } | GraphPattern::Minus { left, right }
        | GraphPattern::LeftJoin { left, right, .. } => node_only(v, left, nodes) && node_only(v, right, nodes),
        GraphPattern::Filter { inner, .. } | GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Project { inner, .. } | GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. } => node_only(v, inner, nodes),
    }
}

/// Variables on which the rows of `p` are unique (no two rows agree on all of them), if provable: a GROUP BY's keys
/// when each is certainly bound inside it (`certainly_bound`: not only by OPTIONAL, one UNION branch, BIND or VALUES
/// with UNDEF) (no keys: one row), kept through FILTER, BIND, DISTINCT/REDUCED and a projection that keeps them; a join of two
/// such inputs is unique on the union (each row is one row of each side).
fn unique_on(p: &GraphPattern) -> Option<Vec<(&str, &GraphPattern)>> {
    match p {
        // The empty group has exactly one solution (the identity of a join of subqueries).
        GraphPattern::Bgp { patterns } if patterns.is_empty() => Some(Vec::new()),
        // Each key must be bound in every group: an unbound key joins with any value, so two groups (UNBOUND and
        // <x>) could meet one <x> group on the other side and give two rows with the same final key.
        GraphPattern::Group { inner, variables, .. } => {
            let bound = super::certainly_bound(inner);
            variables.iter().all(|v| bound.contains(v.as_str())).then(|| variables.iter().map(|v| (v.as_str(), &**inner)).collect())
        }
        GraphPattern::Filter { inner, .. } | GraphPattern::Extend { inner, .. } | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner } => unique_on(inner),
        GraphPattern::Project { inner, variables } => {
            unique_on(inner).filter(|vs| vs.iter().all(|(v, _)| variables.iter().any(|x| x.as_str() == *v)))
        }
        GraphPattern::Join { left, right } => {
            // A key of both sides must be proven a node on both (an unbound left key takes the right one's value).
            let mut vs = unique_on(left)?;
            vs.extend(unique_on(right)?);
            Some(vs)
        }
        _ => None,
    }
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
    numeric: Option<Decimal>,
}

impl<S: Snapshot + ?Sized> Ctx<'_, S> {
    /// `(column, bound)` if `e` is a comparison of a variable with an order-preserving constant. Negations,
    /// disjunctions and anything else are not bounds (they stay ordinary filters).
    fn range_bound(&self, e: &Expression, allow_numeric: bool) -> Option<(usize, Bound)> {
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
        let datatype = ordered_constant(l, allow_numeric)?;
        let col = *self.vars.get(v.as_str())?;
        let numeric = (datatype == NUMERIC_MARKER).then(|| expr::range_decimal(l.as_ref())).flatten();
        Some((col, Bound { side, datatype, text: triplelight_rdf::Term::from(l.clone()).to_string(), numeric }))
    }

    /// The ranged patterns of a BGP: `?s <p> ?d` where some pushed conjuncts bound `?d`, and the
    /// stored literal objects pass the date or numeric ID-order proof. Empty when the switch
    /// is off, inside GRAPH / FROM (the active graph is not `snap`), or nothing applies.
    /// Patterns whose subject or object `first` (the first input row) binds are skipped: they are read through the
    /// bound value, and computing their range would only cost (e.g. once per row of an EXISTS).
    pub(super) fn object_ranges(&self, patterns: &[crate::sparql::TriplePattern], pats: &[[Slot; 3]], filters: &[(&Expression, Vec<usize>)], first: &[TermId]) -> Ranges {
        let mut out = Ranges::new();
        if !self.range_pushdown || filters.is_empty() || self.data.get().is_some() { return out }
        // Only BGPs whose solution order cannot reach the answers (`reorderable_bgps`).
        if !self.range_reorderable.contains(&(patterns.as_ptr() as usize)) { return out }
        let mut bounds: HashMap<usize, Vec<Bound>> = HashMap::new();
        for (e, _) in filters {
            if let Some((col, b)) = self.range_bound(e, self.numeric_range_pushdown) { bounds.entry(col).or_default().push(b) }
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

    /// Conservative ID intervals for a wide-table star, in its column order. Unlike the index path, a dense
    /// interval is useful here: the scan already visits each subject and can reject values before row expansion.
    /// A single interval cannot also include unordered overlay IDs, so any such object leaves that column unbounded.
    pub(super) fn table_intervals(&self, columns: &[(TermId, Slot)], filters: &[(&Expression, Vec<usize>)]) -> Vec<Option<(TermId, TermId)>> {
        let mut out = vec![None; columns.len()];
        // A fixed object is always an exact single-ID interval. Unlike a
        // FILTER range, it does not depend on dictionary text ordering.
        for (i, &(_, object)) in columns.iter().enumerate() {
            if let Slot::Const(Some(id)) = object {
                out[i] = id.0.checked_add(1).map(|next| (id, TermId(next)));
            }
        }
        if !self.range_pushdown || filters.is_empty() || self.data.get().is_some() { return out }
        let mut bounds: HashMap<usize, Vec<Bound>> = HashMap::new();
        for (e, _) in filters {
            if let Some((col, b)) = self.range_bound(e, true) { bounds.entry(col).or_default().push(b) }
        }
        for (i, &(p, object)) in columns.iter().enumerate() {
            let Slot::Var(slot) = object else { continue };
            let Some(bs) = bounds.get(&slot) else { continue };
            let Some((lo, hi, _)) = self.literal_interval(p, bs) else { continue };
            // Delta, scratch and inline terms do not follow the base literal text order. The table scanner has
            // one interval per column, so it cannot include them in a separate unrestricted pass.
            if self.snap.seek(Perm::Pos, &[p], FIRST_UNORDERED).unwrap_or_else(|e| storage_failure(e)).is_none() {
                out[i] = Some((lo, hi));
            }
        }
        out
    }

    fn object_range(&self, p: TermId, bounds: &[Bound]) -> Option<ObjRange> {
        let (lo, hi, literals) = self.literal_interval(p, bounds)?;
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

    fn literal_interval(&self, p: TermId, bounds: &[Bound]) -> Option<(TermId, TermId, Arc<[TermId]>)> {
        let datatype = bounds[0].datatype;
        // Bounds of different datatypes cannot all hold; leave such a conjunction to the filter.
        if bounds.iter().any(|b| b.datatype != datatype) { return None }
        if datatype == NUMERIC_MARKER {
            let literals = self.ordered_literal_objects(p, datatype)?;
            let mut start = 0;
            let mut end = literals.len();
            for b in bounds {
                let index = self.numeric_bound_index(&literals, b);
                match b.side {
                    Side::Lower { .. } => start = start.max(index),
                    Side::Upper { .. } => end = end.min(index),
                }
            }
            let (lo, hi) = if start < end {
                (literals[start], TermId(literals[end - 1].0 + 1))
            } else { (FIRST_LITERAL, FIRST_LITERAL) };
            return Some((lo, hi, literals));
        }
        // The text bounds first: a store that cannot promise them (`None`) skips the object enumeration below.
        let (mut lo, mut hi) = (FIRST_LITERAL, END_LITERAL);
        for b in bounds {
            let strict = match b.side { Side::Lower { strict } => strict, Side::Upper { inclusive } => inclusive };
            let id = self.snap.literal_text_bound(&b.text, strict).unwrap_or_else(|e| storage_failure(e))?;
            match b.side { Side::Lower { .. } => lo = lo.max(id), Side::Upper { .. } => hi = hi.min(id) }
        }
        let literals = self.ordered_literal_objects(p, datatype)?;
        Some((lo, hi, literals))
    }

    /// Binary-search the monotone, cached numeric IDs. The first query pays for the bounded proof; later bounds
    /// decode only O(log n) terms. A lower bound finds the first satisfying value; an upper bound finds the first
    /// value past the upper limit.
    fn numeric_bound_index(&self, ids: &[TermId], bound: &Bound) -> usize {
        let mut left = 0;
        let mut right = ids.len();
        while left < right {
            let mid = left + (right - left) / 2;
            let mut cmp = None;
            self.snap.with_term(ids[mid], &mut |t| {
                if let TermRef::Literal(l) = t {
                    cmp = expr::range_decimal(l).map(|n| n.cmp(&bound.numeric.unwrap()));
                }
            }).unwrap_or_else(|e| storage_failure(e));
            let cmp = cmp.expect("memoized numeric object must compare with integer/decimal bound");
            let before = match bound.side {
                Side::Lower { strict } => cmp.is_lt() || (strict && cmp.is_eq()),
                Side::Upper { inclusive } => cmp.is_lt() || (inclusive && cmp.is_eq()),
            };
            if before { left = mid + 1 } else { right = mid }
        }
        left
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

    /// The stored-literal objects of `p` (only numeric IDs for the numeric key), ascending, if the date canonical-form
    /// or numeric monotonicity proof holds;
    /// `None` otherwise or above `CHECK_LIMIT` objects. Cached in the snapshot's
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
            let mut numeric = (datatype == NUMERIC_MARKER).then(|| vec![None; ids.len()]);
            self.snap.with_terms(&ids, &mut |position, t| {
                let TermRef::Literal(l) = t else { return };
                if datatype == NUMERIC_MARKER {
                    if l.datatype() == xsd::FLOAT || l.datatype() == xsd::DOUBLE { ok = false; return }
                    let Some(n) = expr::range_decimal(l) else { return };
                    numeric.as_mut().unwrap()[position] = Some(n);
                } else if l.datatype().as_str() == datatype && !canonical(datatype, l.value()) { ok = false }
            }).unwrap_or_else(|e| storage_failure(e));
            if let Some(numeric) = numeric {
                let mut previous = None;
                let mut numeric_ids = Vec::new();
                for (id, n) in ids.into_iter().zip(numeric) {
                    let Some(n) = n else { continue };
                    if previous.is_some_and(|prev: Decimal| prev > n) {
                        ok = false;
                        break;
                    }
                    previous = Some(n);
                    numeric_ids.push(id);
                }
                return ok.then(|| numeric_ids.into());
            }
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
