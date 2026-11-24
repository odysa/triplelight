//! RDFS entailment at query time (`Entailment::Rdfs`, task #108): a `Snapshot` view over a graph whose scans, counts
//! and seeks answer the graph's RDFS closure, so the planner and every operator run unchanged on top of it. Nothing is
//! written to the store.
//!
//! # The closure
//!
//! With `sc` = `rdfs:subClassOf`, `sp` = `rdfs:subPropertyOf`, `dom` = `rdfs:domain`, `rng` = `rdfs:range`, the view
//! holds the smallest set of triples that contains the graph and is closed under:
//!
//! - `sc` and `sp` transitivity (rdfs5, rdfs11);
//! - `(s p o), p sp q => (s q o)` (rdfs7);
//! - `(s type C), C sc D => (s type D)` (rdfs9);
//! - `(s p o), p dom C => (s type C)` (rdfs2) and `(s p o), p rng C => (o type C)` for an `o` that is not a literal
//!   (rdfs3; a literal cannot be a subject in RDF 1.1).
//!
//! This is the rule set of Jena's `RDFSRuleReasoner` at level `simple`: no axiomatic triples (`rdfs:Resource`,
//! reflexive `sc`/`sp`, container membership), no rdfs1/4/6/8/10/12/13. Schema triples are read from the graph itself
//! and are not themselves inferred: a property declared a subproperty of `rdfs:subClassOf`, `rdfs:subPropertyOf`,
//! `rdfs:domain`, `rdfs:range` or `rdf:type` does not take part in the schema.
//!
//! Every triple appears once (the view is a set), so an asserted triple and two derivations of it match once.
//!
//! # Access paths
//!
//! A scan with the predicate bound to `q` (not one of the four schema predicates or `rdf:type`) is the union of the
//! graph's scans of `q` and its subproperties, with the predicate column rewritten to `q`; with no subproperty it is the
//! graph's own scan. `rdf:type` with the class bound is the union of the class's and its subclasses' type triples and
//! the subjects (objects) of every property whose domain (range) lies below the class. Other shapes (unbound
//! predicate, unbound class) are materialized from the graph's rows that can contribute. Results other than direct
//! delegations are memoized per view (`MEMO_ROWS`), so a leapfrog seeking one prefix many times pays once.
use triplelight_core::model::{id_tag, Perm, TermId, TripleBatch};
use triplelight_core::storage::{Dictionary, EntailmentProvider, Snapshot, StorageError};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use triplelight_rdf::vocab::{rdf, rdfs};
use triplelight_rdf::{NamedNodeRef, Term, TermRef};

const BATCH: usize = 4096;
/// Memoized rows across all prefixes of one view; past this the memo is emptied before the next insert.
const MEMO_ROWS: usize = 1 << 26;

/// Positions S = 0, P = 1, O = 2.
const S: usize = 0;
const P: usize = 1;
const O: usize = 2;

/// The schema of one graph, closed: strict transitive super- and sub-relations, and per property the classes its
/// domain and range give (closed upward over `sc`, inherited from superproperties).
#[derive(Default)]
pub(crate) struct Schema {
    /// `rdf:type`'s ID, 0 if none: the store's, or (set by the executor when the store has no such IRI) the query's
    /// scratch ID, so that domain and range still type resources in a graph without a single `rdf:type` triple.
    rdf_type: std::sync::atomic::AtomicU64,
    sub_class_of: Option<TermId>,
    sub_property_of: Option<TermId>,
    super_props: HashMap<TermId, Vec<TermId>>,
    sub_props: HashMap<TermId, Vec<TermId>>,
    super_classes: HashMap<TermId, Vec<TermId>>,
    sub_classes: HashMap<TermId, Vec<TermId>>,
    /// Property -> every class `(s p o)` makes `s` an instance of.
    domain_of: HashMap<TermId, Vec<TermId>>,
    /// Property -> every class `(s p o)` makes a non-literal `o` an instance of.
    range_of: HashMap<TermId, Vec<TermId>>,
    /// Class -> properties whose domain (range) classes include it.
    by_domain: HashMap<TermId, Vec<TermId>>,
    by_range: HashMap<TermId, Vec<TermId>>,
}

/// Strict transitive closure of `edges` (a -> every node reachable from a by one or more edges), in discovery order.
fn closure(edges: &[(TermId, TermId)]) -> HashMap<TermId, Vec<TermId>> {
    let mut next: HashMap<TermId, Vec<TermId>> = HashMap::new();
    for &(a, b) in edges { next.entry(a).or_default().push(b) }
    let mut out = HashMap::new();
    for &a in next.keys() {
        let (mut seen, mut order, mut queue) = (HashSet::new(), Vec::new(), VecDeque::from([a]));
        while let Some(x) = queue.pop_front() {
            for &y in next.get(&x).map_or(&[][..], Vec::as_slice) {
                if seen.insert(y) { order.push(y); queue.push_back(y) }
            }
        }
        out.insert(a, order);
    }
    out
}

fn invert(m: &HashMap<TermId, Vec<TermId>>) -> HashMap<TermId, Vec<TermId>> {
    let mut out: HashMap<TermId, Vec<TermId>> = HashMap::new();
    for (&a, bs) in m { for &b in bs { out.entry(b).or_default().push(a) } }
    for v in out.values_mut() { v.sort_unstable(); v.dedup() }
    out
}

fn rows_of(g: &dyn Snapshot, perm: Perm, prefix: &[TermId]) -> Result<Vec<[TermId; 3]>, StorageError> {
    let mut out = Vec::new();
    for b in g.scan(perm, prefix) {
        let b = b?;
        out.extend((0..b.len()).map(|i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]]));
    }
    Ok(out)
}

impl Schema {
    pub(crate) fn of(g: &dyn Snapshot) -> Result<Schema, StorageError> {
        let iri = |n: NamedNodeRef<'_>| g.lookup(&Term::NamedNode(n.into_owned()));
        let (rdf_type, sub_class_of, sub_property_of, domain, range) =
            (iri(rdf::TYPE)?, iri(rdfs::SUB_CLASS_OF)?, iri(rdfs::SUB_PROPERTY_OF)?, iri(rdfs::DOMAIN)?, iri(rdfs::RANGE)?);
        // (subject, object) pairs of one predicate, from its PSO range (key order: p, s, o).
        let pairs = |p: Option<TermId>| -> Result<Vec<(TermId, TermId)>, StorageError> {
            let Some(p) = p else { return Ok(Vec::new()) };
            Ok(rows_of(g, Perm::Pso, &[p])?.into_iter().map(|r| (r[1], r[2])).collect())
        };
        let super_classes = closure(&pairs(sub_class_of)?);
        let super_props = closure(&pairs(sub_property_of)?);
        let (sub_classes, sub_props) = (invert(&super_classes), invert(&super_props));
        // Per property, its declared classes and those of its superproperties, closed upward.
        let inherited = |declared: Vec<(TermId, TermId)>| -> HashMap<TermId, Vec<TermId>> {
            let mut direct: HashMap<TermId, Vec<TermId>> = HashMap::new();
            for (p, c) in declared { direct.entry(p).or_default().push(c) }
            let mut props: HashSet<TermId> = direct.keys().copied().collect();
            for p in direct.keys() { props.extend(sub_props.get(p).into_iter().flatten().copied()) }
            let mut out = HashMap::new();
            for p in props {
                let mut classes: Vec<TermId> = Vec::new();
                for q in std::iter::once(&p).chain(super_props.get(&p).into_iter().flatten()) {
                    for &c in direct.get(q).into_iter().flatten() {
                        classes.push(c);
                        classes.extend(super_classes.get(&c).into_iter().flatten().copied());
                    }
                }
                classes.sort_unstable();
                classes.dedup();
                if !classes.is_empty() { out.insert(p, classes); }
            }
            out
        };
        let domain_of = inherited(pairs(domain)?);
        let range_of = inherited(pairs(range)?);
        let (by_domain, by_range) = (invert(&domain_of), invert(&range_of));
        let rdf_type = std::sync::atomic::AtomicU64::new(rdf_type.map_or(0, |t| t.0));
        Ok(Schema { rdf_type, sub_class_of, sub_property_of, super_props, sub_props, super_classes, sub_classes, domain_of, range_of, by_domain, by_range })
    }

    pub(crate) fn rdf_type(&self) -> Option<TermId> {
        let t = self.rdf_type.load(std::sync::atomic::Ordering::Relaxed);
        (t != 0).then_some(TermId(t))
    }

    /// No rule can add a triple: the view is the graph.
    fn is_empty(&self) -> bool {
        self.super_props.is_empty() && self.super_classes.is_empty() && self.domain_of.is_empty() && self.range_of.is_empty()
    }

    /// The predicates whose triples rewrite to `q` (rdfs7): `q` and its subproperties.
    fn below_prop(&self, q: TermId) -> impl Iterator<Item = TermId> + '_ {
        std::iter::once(q).chain(self.sub_props.get(&q).into_iter().flatten().copied())
    }
}

fn is_schema_closure(s: &Schema, p: TermId) -> bool { Some(p) == s.sub_class_of || Some(p) == s.sub_property_of }

/// An RDF term that may be a subject: not a literal (by ID tag; delta and scratch IDs are resolved).
fn may_be_subject(g: &dyn Snapshot, id: TermId) -> Result<bool, StorageError> {
    Ok(match id_tag::of(id) {
        id_tag::IRI | id_tag::BLANK_NODE | id_tag::TRIPLE_TERM => true,
        id_tag::LITERAL | id_tag::INLINE => false,
        _ => {
            let mut literal = false;
            g.with_term(id, &mut |t| literal = matches!(t, TermRef::Literal(_)))?;
            !literal
        }
    })
}

/// `spo` in the key order of `perm` (`Perm::order()[j]` is the component at key position `j`).
fn key(perm: Perm, spo: [TermId; 3]) -> [TermId; 3] { perm.order().map(|c| spo[c]) }

/// A key-order row of `perm` as (s, p, o).
fn spo(perm: Perm, k: [TermId; 3]) -> [TermId; 3] {
    let mut t = [TermId(0); 3];
    for (j, c) in perm.order().into_iter().enumerate() { t[c] = k[j] }
    t
}

/// The graph `base` with its RDFS closure (module docs).
struct Entailed<'a> {
    base: &'a dyn Snapshot,
    schema: Schema,
    memo: Mutex<(usize, HashMap<(Perm, Vec<TermId>), Arc<Vec<[TermId; 3]>>>)>,
}

impl<'a> Entailed<'a> {
    fn new(base: &'a dyn Snapshot) -> Result<Self, StorageError> {
        Ok(Entailed { base, schema: Schema::of(base)?, memo: Mutex::new((0, HashMap::new())) })
    }

    /// Names `rdf:type` by `id` when the graph's dictionary has no such IRI (`Schema::rdf_type`); before any scan.
    fn set_rdf_type(&self, id: TermId) {
        if self.schema.rdf_type().is_none() { self.schema.rdf_type.store(id.0, std::sync::atomic::Ordering::Relaxed) }
    }

    /// The bound S, P, O of `prefix` in `perm`'s key order.
    fn bound(perm: Perm, prefix: &[TermId]) -> [Option<TermId>; 3] {
        let mut b = [None; 3];
        for (j, &v) in prefix.iter().enumerate() { b[perm.order()[j]] = Some(v) }
        b
    }

    /// `None` when `perm`/`prefix` is answered by the graph as is: no rule can add a matching triple.
    fn delegates(&self, perm: Perm, prefix: &[TermId]) -> bool {
        if self.schema.is_empty() { return true }
        match Self::bound(perm, prefix)[P] {
            Some(q) => Some(q) != self.schema.rdf_type() && !is_schema_closure(&self.schema, q) && !self.schema.sub_props.contains_key(&q),
            None => false,
        }
    }

    /// The view's rows for `perm`/`prefix` in key order, distinct (memoized).
    fn rows(&self, perm: Perm, prefix: &[TermId]) -> Result<Arc<Vec<[TermId; 3]>>, StorageError> {
        let k = (perm, prefix.to_vec());
        if let Some(r) = self.memo.lock().unwrap().1.get(&k) { return Ok(Arc::clone(r)) }
        let b = Self::bound(perm, prefix);
        let mut out: Vec<[TermId; 3]> = self.matching(b)?.into_iter().map(|t| key(perm, t)).collect();
        out.sort_unstable();
        out.dedup();
        let out = Arc::new(out);
        let mut memo = self.memo.lock().unwrap();
        if memo.0 + out.len() > MEMO_ROWS { memo.1.clear(); memo.0 = 0 }
        memo.0 += out.len();
        memo.1.insert(k, Arc::clone(&out));
        Ok(out)
    }

    /// Every entailed (s, p, o) matching the bound components (duplicates allowed).
    fn matching(&self, b: [Option<TermId>; 3]) -> Result<Vec<[TermId; 3]>, StorageError> {
        let s = &self.schema;
        let keep = |t: &[TermId; 3]| (0..3).all(|c| b[c].is_none_or(|v| v == t[c]));
        let mut out: Vec<[TermId; 3]> = Vec::new();
        match b[P] {
            Some(q) if Some(q) == s.rdf_type() => {
                for (x, c) in self.types(b[S], b[O])? { out.push([x, q, c]) }
            }
            Some(q) if is_schema_closure(s, q) => out.extend(self.closure_triples(q).filter(keep)),
            Some(q) => {
                for p in s.below_prop(q) {
                    let mut pre = b;
                    pre[P] = Some(p);
                    for mut t in self.base_matching(pre)? { t[P] = q; out.push(t) }
                }
            }
            None => {
                // Base triples, their superproperty copies, the schema closures and the types, restricted to the bound
                // subject or object (or all of them).
                for t in self.base_matching(b)? {
                    out.push(t);
                    for &q in s.super_props.get(&t[P]).into_iter().flatten() { out.push([t[S], q, t[O]]) }
                }
                for q in [s.sub_class_of, s.sub_property_of].into_iter().flatten() { out.extend(self.closure_triples(q).filter(keep)) }
                if let Some(ty) = s.rdf_type() {
                    for (x, c) in self.types(b[S], b[O])? { out.push([x, ty, c]) }
                }
            }
        }
        out.retain(keep);
        Ok(out)
    }

    /// The graph's triples matching `b` (any perm with the bound components as its prefix).
    fn base_matching(&self, b: [Option<TermId>; 3]) -> Result<Vec<[TermId; 3]>, StorageError> {
        let perm = match (b[S].is_some(), b[P].is_some(), b[O].is_some()) {
            (_, true, true) => Perm::Pos,
            (true, true, false) => Perm::Spo,
            (true, false, true) => Perm::Sop,
            (true, false, false) => Perm::Spo,
            (false, true, false) => Perm::Pso,
            (false, false, true) => Perm::Osp,
            (false, false, false) => Perm::Spo,
        };
        let prefix: Vec<TermId> = perm.order().into_iter().map_while(|c| b[c]).collect();
        let mut out = Vec::new();
        for r in rows_of(self.base, perm, &prefix)? {
            let t = spo(perm, r);
            if (0..3).all(|c| b[c].is_none_or(|v| v == t[c])) { out.push(t) }
        }
        Ok(out)
    }

    /// `(a q b)` for the transitive closure of `q` (`sc` or `sp`), which includes the graph's own `q` triples.
    fn closure_triples(&self, q: TermId) -> impl Iterator<Item = [TermId; 3]> + '_ {
        let m = if Some(q) == self.schema.sub_class_of { &self.schema.super_classes } else { &self.schema.super_props };
        m.iter().flat_map(move |(&a, bs)| bs.iter().map(move |&b| [a, q, b]))
    }

    /// Entailed (instance, class) pairs with the instance and/or class bound.
    fn types(&self, x: Option<TermId>, c: Option<TermId>) -> Result<Vec<(TermId, TermId)>, StorageError> {
        let s = &self.schema;
        // Without `rdf:type` in the dictionary no type triple can be named by a query.
        let Some(ty) = s.rdf_type() else { return Ok(Vec::new()) };
        let up = |c: TermId| std::iter::once(c).chain(s.super_classes.get(&c).into_iter().flatten().copied());
        let mut out = Vec::new();
        match (x, c) {
            (Some(x), _) => {
                let mut classes: Vec<TermId> = Vec::new();
                for r in rows_of(self.base, Perm::Spo, &[x])? {
                    if r[1] == ty { classes.extend(up(r[2])) }
                    classes.extend(s.domain_of.get(&r[1]).into_iter().flatten().copied());
                }
                if may_be_subject(self.base, x)? {
                    // OSP key order: o, s, p.
                    for r in rows_of(self.base, Perm::Osp, &[x])? { classes.extend(s.range_of.get(&r[2]).into_iter().flatten().copied()) }
                }
                classes.sort_unstable();
                classes.dedup();
                out.extend(classes.into_iter().filter(|&k| c.is_none_or(|c| c == k)).map(|k| (x, k)));
            }
            (None, Some(c)) => { for i in self.instances(c)? { out.push((i, c)) } }
            (None, None) => {
                let mut classes: HashSet<TermId> = HashSet::new();
                // POS key order: p, o, s; distinct type objects.
                for r in rows_of(self.base, Perm::Pos, &[ty])? { classes.extend(up(r[1])) }
                classes.extend(s.by_domain.keys().copied());
                classes.extend(s.by_range.keys().copied());
                for c in classes { for i in self.instances(c)? { out.push((i, c)) } }
            }
        }
        Ok(out)
    }

    /// Every entailed instance of `c` (duplicates allowed).
    fn instances(&self, c: TermId) -> Result<Vec<TermId>, StorageError> {
        let s = &self.schema;
        let Some(ty) = s.rdf_type() else { return Ok(Vec::new()) };
        let mut out = Vec::new();
        for k in std::iter::once(c).chain(s.sub_classes.get(&c).into_iter().flatten().copied()) {
            for b in self.base.scan(Perm::Pos, &[ty, k]) { out.extend_from_slice(&b?.cols[2]) }
        }
        for &p in s.by_domain.get(&c).into_iter().flatten() {
            // PSO key order: p, s, o.
            for b in self.base.scan(Perm::Pso, &[p]) { out.extend_from_slice(&b?.cols[1]) }
        }
        for &p in s.by_range.get(&c).into_iter().flatten() {
            // POS key order: p, o, s.
            for b in self.base.scan(Perm::Pos, &[p]) {
                let b = b?;
                for &o in &b.cols[1] { if may_be_subject(self.base, o)? { out.push(o) } }
            }
        }
        Ok(out)
    }
}

impl Dictionary for Entailed<'_> {
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError> { self.base.lookup(term) }
    fn literal_text_bound(&self, text: &str, strict: bool) -> Result<Option<TermId>, StorageError> { self.base.literal_text_bound(text, strict) }
    fn resolve(&self, id: TermId) -> Result<Term, StorageError> { self.base.resolve(id) }
    fn with_term(&self, id: TermId, f: &mut dyn FnMut(TermRef<'_>)) -> Result<(), StorageError> { self.base.with_term(id, f) }
    fn with_terms(&self, ids: &[TermId], f: &mut dyn FnMut(usize, TermRef<'_>)) -> Result<(), StorageError> { self.base.with_terms(ids, f) }
    #[cfg(feature = "rdf-12")]
    fn triple_term_parts(&self, id: TermId) -> Result<Option<[TermId; 3]>, StorageError> { self.base.triple_term_parts(id) }
}

impl Snapshot for Entailed<'_> {
    fn has_rdf12_terms(&self) -> bool { self.base.has_rdf12_terms() }
    fn scan<'b>(&'b self, perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'b> {
        if self.delegates(perm, prefix) { return self.base.scan(perm, prefix) }
        let rows = match self.rows(perm, prefix) { Ok(r) => r, Err(e) => return Box::new(std::iter::once(Err(e))) };
        let mut at = 0;
        Box::new(std::iter::from_fn(move || {
            if at >= rows.len() { return None }
            let end = (at + BATCH).min(rows.len());
            let mut b = TripleBatch::default();
            for r in &rows[at..end] { for (col, v) in b.cols.iter_mut().zip(r) { col.push(*v) } }
            at = end;
            Some(Ok(b))
        }))
    }
    fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> {
        if self.delegates(perm, prefix) { return self.base.count(perm, prefix) }
        Ok(self.rows(perm, prefix)?.len() as u64)
    }
    fn seek(&self, perm: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> {
        if self.delegates(perm, prefix) { return self.base.seek(perm, prefix, target) }
        if prefix.len() >= 3 { return Ok(None) }
        let rows = self.rows(perm, prefix)?;
        let mut lo = prefix.to_vec();
        lo.push(target);
        let i = rows.partition_point(|r| r[..lo.len()] < lo[..]);
        Ok(rows.get(i).filter(|r| r[..prefix.len()] == *prefix).map(|r| r[prefix.len()]))
    }
}

/// The RDFS entailment provider.
pub struct Rdfs;

impl EntailmentProvider for Rdfs {
    fn virtual_predicates(&self) -> &'static [NamedNodeRef<'static>] {
        &[rdf::TYPE]
    }

    fn graph<'a>(
        &self,
        source: &'a dyn Snapshot,
        virtual_ids: &[(NamedNodeRef<'static>, TermId)],
    ) -> Result<Box<dyn Snapshot + 'a>, StorageError> {
        let view = Entailed::new(source)?;
        if let Some((_, id)) = virtual_ids.iter().find(|(predicate, _)| *predicate == rdf::TYPE) {
            view.set_rdf_type(*id);
        }
        Ok(Box::new(view))
    }
}
