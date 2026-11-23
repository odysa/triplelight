//! SPARQL 1.2 triple-term patterns (W3C SPARQL 1.2 Query Language WD 2026-11-19, §18.1.3) evaluated on term IDs.
//!
//! A triple pattern whose object is a triple-term pattern `<<( s p o )>>` is lowered in one of two ways:
//! - fully ground (no variables or blank nodes at any depth): the triple term is a constant, looked up in the
//!   dictionary like any other (absent: no match);
//! - otherwise: the object slot becomes a hidden internal variable (`_:t<<( … )>>`, named by the pattern's text so
//!   every occurrence of one pattern shares one column), the BGP is evaluated as usual, and each solution is then
//!   unified with the nested pattern: the bound ID must be a triple term (`Dictionary::triple_term_parts`), whose
//!   subject, predicate and object bind the nested variables (blank nodes act as variables), match the nested
//!   constants and agree with variables already bound, recursively for nested triple terms.
//!
//! A triple-term pattern in subject position never matches RDF data (RDF 1.2 has no subject triple terms).
//! Patterns without triple terms never reach this module, so the ordinary BGP fast paths are unchanged.
use super::{storage_failure, Ctx, Row, UNBOUND};
use crate::model::TermId;
use crate::sparql::{Expression, NamedNodePattern, TermPattern, TriplePattern};
use crate::storage::Snapshot;
use std::collections::HashSet;
use std::fmt::Write;
use triplelight_rdf::{BlankNode, NamedOrBlankNode, Term, Triple};

/// Whether `t` has a triple-term pattern in subject or object position.
pub(crate) fn has_triple_term(t: &TriplePattern) -> bool {
    matches!(t.subject, TermPattern::Triple(_)) || matches!(t.object, TermPattern::Triple(_))
}

/// Whether the triple-term pattern `t` has no variable and no blank node at any depth.
pub(crate) fn is_ground(t: &TriplePattern) -> bool {
    fn ground(tp: &TermPattern) -> bool {
        match tp {
            TermPattern::Variable(_) | TermPattern::BlankNode(_) => false,
            TermPattern::Triple(t) => is_ground(t),
            TermPattern::NamedNode(_) | TermPattern::Literal(_) => true,
        }
    }
    ground(&t.subject) && matches!(t.predicate, NamedNodePattern::NamedNode(_)) && ground(&t.object)
}

/// The triple term a ground triple-term pattern denotes; `None` if it is not ground or denotes no RDF term (a literal
/// or triple-term subject), so it can match nothing.
pub(crate) fn ground_term(t: &TriplePattern) -> Option<Term> {
    fn term(tp: &TermPattern) -> Option<Term> {
        match tp {
            TermPattern::NamedNode(n) => Some(n.clone().into()),
            TermPattern::Literal(l) => Some(l.clone().into()),
            TermPattern::Triple(t) => ground_term(t),
            TermPattern::Variable(_) | TermPattern::BlankNode(_) => None,
        }
    }
    let s = NamedOrBlankNode::try_from(term(&t.subject)?).ok()?;
    let NamedNodePattern::NamedNode(p) = &t.predicate else { return None };
    Some(Triple::new(s, p.clone(), term(&t.object)?).into())
}

fn key(tp: &TermPattern, out: &mut String) {
    match tp {
        TermPattern::Variable(v) => { out.push('?'); out.push_str(v.as_str()) }
        TermPattern::BlankNode(b) => { out.push_str("_:"); out.push_str(b.as_str()) }
        TermPattern::NamedNode(n) => { let _ = write!(out, "{n}"); }
        TermPattern::Literal(l) => { let _ = write!(out, "{l}"); }
        TermPattern::Triple(t) => triple_key(t, out),
    }
}
fn triple_key(t: &TriplePattern, out: &mut String) {
    out.push_str("<<( ");
    key(&t.subject, out);
    out.push(' ');
    match &t.predicate {
        NamedNodePattern::Variable(v) => { out.push('?'); out.push_str(v.as_str()) }
        NamedNodePattern::NamedNode(n) => { let _ = write!(out, "{n}"); }
    }
    out.push(' ');
    key(&t.object, out);
    out.push_str(" )>>");
}

/// The hidden variable standing for a non-ground triple-term pattern in a BGP or path slot (`None` when ground). It is
/// an internal variable (`INTERNAL_PREFIX`), so it never appears in `SELECT *` or results.
pub(crate) fn hidden_var(t: &TriplePattern) -> Option<String> {
    if is_ground(t) { return None }
    let mut s = format!("{}t", crate::sparql::INTERNAL_PREFIX);
    triple_key(t, &mut s);
    Some(s)
}

/// Calls `f` with the name of every variable inside `tp` (recursively, subject-predicate-object order), and with the
/// internal name of every blank node (`_:b<label>`, as top-level pattern blank nodes are named) when `blanks`.
pub(crate) fn term_names(tp: &TermPattern, blanks: bool, f: &mut dyn FnMut(String)) {
    match tp {
        TermPattern::Variable(v) => f(v.as_str().to_string()),
        TermPattern::BlankNode(b) => if blanks { f(format!("_:b{}", b.as_str())) },
        TermPattern::Triple(t) => {
            term_names(&t.subject, blanks, f);
            if let NamedNodePattern::Variable(v) = &t.predicate { f(v.as_str().to_string()) }
            term_names(&t.object, blanks, f);
        }
        TermPattern::NamedNode(_) | TermPattern::Literal(_) => {}
    }
}

/// Whether `tp` is or contains a blank node.
pub(crate) fn has_blank(tp: &TermPattern) -> bool {
    match tp {
        TermPattern::BlankNode(_) => true,
        TermPattern::Triple(t) => has_blank(&t.subject) || has_blank(&t.object),
        _ => false,
    }
}

/// Every blank-node label in `t`, including inside triple terms.
pub(crate) fn blank_labels(t: &Term, out: &mut HashSet<String>) {
    match t {
        Term::BlankNode(b) => { out.insert(b.as_str().to_string()); }
        Term::Triple(tr) => {
            if let NamedOrBlankNode::BlankNode(b) = &tr.subject { out.insert(b.as_str().to_string()); }
            blank_labels(&tr.object, out);
        }
        _ => {}
    }
}

impl<S: Snapshot + ?Sized> Ctx<'_, S> {
    /// A BGP with at least one triple-term pattern (see the module documentation). `filters` are applied once the
    /// nested variables are bound, after unification.
    pub(crate) fn eval_bgp_triple_terms(&self, patterns: &[TriplePattern], rows: Vec<Row>, filters: &[(&Expression, Vec<usize>)]) -> Vec<Row> {
        if patterns.iter().any(|t| matches!(t.subject, TermPattern::Triple(_))) { return Vec::new() }
        let open: Vec<(&TriplePattern, usize)> = patterns.iter().filter_map(|t| match &t.object {
            TermPattern::Triple(tt) => hidden_var(tt).map(|h| (&**tt, self.vars[&h])),
            _ => None,
        }).collect();
        // Only ground triple terms: they are plain constants of the ordinary evaluation.
        if open.is_empty() { return self.eval_bgp_slots(patterns, rows, filters) }
        let mut out = self.eval_bgp_slots(patterns, rows, &[]);
        out.retain_mut(|r| open.iter().all(|&(tt, h)| { let id = r[h]; self.unify_triple(id, tt, r) }));
        if filters.is_empty() || out.is_empty() { return out }
        let w = self.vars.len();
        let mut flat: Vec<TermId> = out.concat();
        let all: HashSet<usize> = (0..w).collect();
        self.apply_filters(&mut flat, w, filters, &all, &mut vec![false; filters.len()]);
        flat.chunks_exact(w).map(<[TermId]>::to_vec).collect()
    }

    /// Keeps the path solutions in `rows` whose non-ground triple-term endpoints unify with the bound triple terms.
    pub(crate) fn unify_path_endpoints(&self, endpoints: [&TermPattern; 2], rows: &mut Vec<Row>) {
        let open: Vec<(&TriplePattern, usize)> = endpoints.iter().filter_map(|tp| match tp {
            TermPattern::Triple(tt) => hidden_var(tt).map(|h| (&**tt, self.vars[&h])),
            _ => None,
        }).collect();
        if open.is_empty() { return }
        rows.retain_mut(|r| open.iter().all(|&(tt, h)| { let id = r[h]; self.unify_triple(id, tt, r) }));
    }

    /// Subject, predicate and object IDs of triple term `id`, or `None` if `id` is not a triple term. Decoded through
    /// the dictionary: ID tag bits are not a reliable kind test across stores (overlay deltas use their own tag).
    fn triple_parts(&self, id: TermId) -> Option<[TermId; 3]> {
        if id == UNBOUND { return None }
        if id.0 >> 60 == 0xe {
            let Term::Triple(t) = self.resolve(id) else { return None };
            let t = *t;
            return Some([self.intern(&t.subject.into()), self.intern(&t.predicate.into()), self.intern(&t.object)]);
        }
        self.snap.triple_term_parts(id).unwrap_or_else(|e| storage_failure(e))
    }

    /// Unifies triple term `id` with the triple-term pattern `t`, binding its variables in `row`. On `false` the row
    /// may be partially bound and must be discarded.
    pub(crate) fn unify_triple(&self, id: TermId, t: &TriplePattern, row: &mut Row) -> bool {
        let Some([s, p, o]) = self.triple_parts(id) else { return false };
        let pred_ok = match &t.predicate {
            NamedNodePattern::Variable(v) => bind(row, self.var(v), p),
            NamedNodePattern::NamedNode(n) => self.intern(&n.clone().into()) == p,
        };
        pred_ok && self.unify_term(s, &t.subject, row) && self.unify_term(o, &t.object, row)
    }

    fn unify_term(&self, id: TermId, tp: &TermPattern, row: &mut Row) -> bool {
        match tp {
            TermPattern::Variable(v) => bind(row, self.var(v), id),
            TermPattern::BlankNode(b) => bind(row, self.vars[&format!("_:b{}", b.as_str())], id),
            // `intern` gives the stored ID of a stored term (cached per query), so ID equality is term equality.
            TermPattern::NamedNode(n) => self.intern(&n.clone().into()) == id,
            TermPattern::Literal(l) => self.intern(&l.clone().into()) == id,
            TermPattern::Triple(t) => {
                if is_ground(t) { ground_term(t).is_some_and(|term| self.intern(&term) == id) } else { self.unify_triple(id, t, row) }
            }
        }
    }

    /// A CONSTRUCT / template triple term instantiated under `row`; `blank` maps a template blank node to its node for
    /// this solution. `None` if a slot is unbound or ill-typed (the template triple is then skipped).
    pub(crate) fn instantiate_triple(&self, t: &TriplePattern, row: &Row, blank: &dyn Fn(&BlankNode) -> Term) -> Option<Term> {
        let term = |tp: &TermPattern| -> Option<Term> {
            match tp {
                TermPattern::BlankNode(b) => Some(blank(b)),
                TermPattern::Triple(t) => self.instantiate_triple(t, row, blank),
                tp => self.term_slot(tp, row).and_then(|id| (id != UNBOUND).then(|| self.resolve(id))),
            }
        };
        let s = NamedOrBlankNode::try_from(term(&t.subject)?).ok()?;
        let p = match &t.predicate {
            NamedNodePattern::NamedNode(n) => n.clone(),
            NamedNodePattern::Variable(v) => match self.var_value(v, row)? { Term::NamedNode(n) => n, _ => return None },
        };
        Some(Triple::new(s, p, term(&t.object)?).into())
    }
}

#[inline]
fn bind(row: &mut Row, i: usize, id: TermId) -> bool {
    if row[i] == UNBOUND { row[i] = id; true } else { row[i] == id }
}
