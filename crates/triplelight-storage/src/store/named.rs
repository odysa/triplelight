//! Named graphs over an overlay (task #36, step 1): the catalog, graph views, and ordered dataset updates.
//!
//! Every named graph is kept in memory as six sorted full-row permutations over the overlay's dictionary (base terms
//! plus delta terms), so a term has one ID in every graph. The default graph is the overlay itself and is unchanged.
//! The disk base does not hold named graphs yet (step 3), and the WAL does not record them yet (step 2): until then
//! only the in-memory store accepts named-graph updates.
use super::{DictRef, Delta, Overlay, DELTA_TAG, key, range, slot};
use crate::model::{Change, DatasetChange, GraphScope, GraphTarget, Perm, TermId, TripleBatch};
use crate::storage::{Dictionary, Snapshot, StorageError};
use oxrdf::{NamedNode, Term, TermRef, Triple};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use triplelight_core::storage::DatasetView;

/// Above this many named-graph rows in the graphs being compared, `shares_blank_nodes` answers "cannot tell".
const BLANK_CHECK_LIMIT: usize = 4_000_000;

/// Why a dataset update was refused. Carried inside the returned `std::io::Error` (kind `AlreadyExists` or
/// `NotFound`), so callers can downcast it with `get_ref().and_then(|e| e.downcast_ref::<GraphStateError>())`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphStateError {
    /// `CREATE GRAPH` (without `SILENT`) of a graph already in the catalog.
    AlreadyExists(NamedNode),
    /// `DROP`/`CLEAR GRAPH` (without `SILENT`) of a graph not in the catalog.
    NotFound(NamedNode),
}

impl std::fmt::Display for GraphStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphStateError::AlreadyExists(g) => write!(f, "graph {g} already exists"),
            GraphStateError::NotFound(g) => write!(f, "graph {g} does not exist"),
        }
    }
}
impl std::error::Error for GraphStateError {}

impl GraphStateError {
    fn into_io(self) -> std::io::Error {
        let kind = match self {
            GraphStateError::AlreadyExists(_) => std::io::ErrorKind::AlreadyExists,
            GraphStateError::NotFound(_) => std::io::ErrorKind::NotFound,
        };
        std::io::Error::new(kind, self)
    }
}

/// One graph's triples in all six key orders (index `slot(perm)`), each sorted and deduplicated.
#[derive(Default)]
struct Rows([Vec<[TermId; 3]>; 6]);

impl Rows {
    fn from_spo(spo: &BTreeSet<[TermId; 3]>) -> Rows {
        let mut rows: [Vec<[TermId; 3]>; 6] = Default::default();
        for p in Perm::ALL {
            let mut v: Vec<[TermId; 3]> = spo.iter().map(|&t| key(p, t)).collect();
            v.sort_unstable();
            rows[slot(p)] = v;
        }
        Rows(rows)
    }
    fn len(&self) -> usize { self.0[0].len() }
}

/// A read view of one named graph: a `Snapshot` over the shared dictionary.
pub struct NamedGraph {
    base: Arc<dyn Snapshot>,
    delta: Arc<Delta>,
    rows: Arc<Rows>,
}

impl NamedGraph {
    fn dict(&self) -> DictRef<'_> { DictRef { base: &*self.base, delta: &self.delta } }
    fn rows(&self, perm: Perm) -> &[[TermId; 3]] { &self.rows.0[slot(perm)] }
}

impl Dictionary for NamedGraph {
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError> { self.dict().lookup(term) }
    fn resolve(&self, id: TermId) -> Result<Term, StorageError> { self.dict().resolve(id) }
    fn with_term(&self, id: TermId, f: &mut dyn FnMut(TermRef<'_>)) -> Result<(), StorageError> { self.dict().with_term(id, f) }
    fn with_terms(&self, ids: &[TermId], f: &mut dyn FnMut(usize, TermRef<'_>)) -> Result<(), StorageError> {
        self.dict().with_terms(ids, f)
    }
}

impl Snapshot for NamedGraph {
    fn scan<'a>(&'a self, perm: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a> {
        let mut chunks = range(self.rows(perm), prefix).chunks(4096);
        Box::new(std::iter::from_fn(move || {
            let chunk = chunks.next()?;
            let mut b = TripleBatch::default();
            for r in chunk {
                for (column, &value) in b.cols.iter_mut().zip(r) { column.push(value) }
            }
            Some(Ok(b))
        }))
    }
    fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, StorageError> {
        Ok(range(self.rows(perm), prefix).len() as u64)
    }
    fn seek(&self, perm: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> {
        let k = prefix.len();
        if k >= 3 { return Ok(None) }
        let rows = range(self.rows(perm), prefix);
        Ok(rows.get(rows.partition_point(|r| r[k] < target)).map(|r| r[k]))
    }
    fn matching_rows(&self, perm: Perm, prefix: &[TermId]) -> Option<&[[TermId; 3]]> {
        Some(range(self.rows(perm), prefix))
    }
}

/// The named graphs of one overlay, by graph-name ID.
#[derive(Default)]
pub struct Catalog(BTreeMap<TermId, NamedGraph>);

impl Catalog {
    pub fn is_empty(&self) -> bool { self.0.is_empty() }
}

impl DatasetView for Overlay {
    fn default_graph(&self) -> &dyn Snapshot { self }
    fn graph(&self, name: TermId) -> Result<Option<&dyn Snapshot>, StorageError> {
        Ok(self.named.0.get(&name).map(|g| g as &dyn Snapshot))
    }
    fn graph_names(&self) -> Result<Vec<TermId>, StorageError> { Ok(self.named.0.keys().copied().collect()) }
    fn shares_blank_nodes(&self, graphs: &[TermId]) -> Result<Option<bool>, StorageError> {
        let mut graphs: Vec<TermId> = graphs.to_vec();
        graphs.sort_unstable();
        graphs.dedup();
        let views: Vec<&NamedGraph> = graphs.iter().filter_map(|g| self.named.0.get(g)).collect();
        if views.iter().map(|v| v.rows.len()).sum::<usize>() > BLANK_CHECK_LIMIT { return Ok(None) }
        let mut kind: HashMap<TermId, bool> = HashMap::new();
        let mut owner: HashMap<TermId, usize> = HashMap::new();
        for (i, view) in views.iter().enumerate() {
            // Blank nodes occur only as subjects and objects.
            for r in view.rows(Perm::Spo) {
                for id in [r[0], r[2]] {
                    let blank = match kind.get(&id) {
                        Some(&b) => b,
                        None => {
                            let mut b = false;
                            self.with_term(id, &mut |t| b = matches!(t, TermRef::BlankNode(_)))?;
                            kind.insert(id, b);
                            b
                        }
                    };
                    if blank && *owner.entry(id).or_insert(i) != i { return Ok(Some(true)) }
                }
            }
        }
        Ok(Some(false))
    }
}

/// Calls `f` with the rows only in `after` (insertions) and only in `before` (deletions); both sorted.
fn diff(before: &[[TermId; 3]], after: &[[TermId; 3]], f: &mut dyn FnMut([TermId; 3], Change)) {
    let (mut i, mut j) = (0, 0);
    while i < before.len() || j < after.len() {
        match (before.get(i), after.get(j)) {
            (Some(a), Some(b)) if a == b => { i += 1; j += 1 }
            (Some(a), Some(b)) if a < b => { f(*a, Change::Delete); i += 1 }
            (Some(a), None) => { f(*a, Change::Delete); i += 1 }
            (_, Some(b)) => { f(*b, Change::Insert); j += 1 }
            (None, None) => unreachable!(),
        }
    }
}

/// A named graph being updated: still shared with the old overlay, or copied out as a set of SPO rows.
enum Pending {
    Shared(Arc<Rows>),
    Owned(BTreeSet<[TermId; 3]>),
}

impl Pending {
    fn len(&self) -> usize { match self { Pending::Shared(r) => r.len(), Pending::Owned(s) => s.len() } }
    fn owned(&mut self) -> &mut BTreeSet<[TermId; 3]> {
        if let Pending::Shared(rows) = self {
            *self = Pending::Owned(rows.0[slot(Perm::Spo)].iter().copied().collect());
        }
        match self { Pending::Owned(s) => s, Pending::Shared(_) => unreachable!() }
    }
}

/// The working state of one dataset update. Nothing is published unless every step succeeds.
struct Work<'a> {
    base: &'a Arc<dyn Snapshot>,
    d: Delta,
    named: BTreeMap<TermId, Pending>,
    changed: usize,
    /// Default-graph rows whose visibility changed at some step, with their visibility before the update.
    touched: HashMap<[TermId; 3], bool>,
    /// Whether the default graph was emptied (then the net change is found by comparing whole scans).
    cleared: bool,
    /// Default-graph rows whose visibility changed since the delta was last brought up to date, with their current
    /// visibility. Applied to the delta in one sorted merge per order (`flush_default`), so a large update costs
    /// O(n log n) instead of one sorted-vector insertion per triple.
    pending: HashMap<[TermId; 3], bool>,
}

impl Work<'_> {
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError> {
        Ok(self.base.lookup(term)?.or_else(|| self.d.terms.get(term).copied()))
    }
    fn intern(&mut self, term: Term) -> Result<TermId, StorageError> {
        if let Some(id) = self.lookup(&term)? { return Ok(id) }
        let id = TermId((DELTA_TAG << 60) | self.d.rev.len() as u64);
        self.d.terms.insert(term.clone(), id);
        self.d.rev.push(term);
        Ok(id)
    }
    fn ids(&mut self, t: &Triple, insert: bool) -> Result<Option<[TermId; 3]>, StorageError> {
        let terms: [Term; 3] = [t.subject.clone().into(), t.predicate.clone().into(), t.object.clone()];
        let mut spo = [TermId(0); 3];
        for (slot, term) in spo.iter_mut().zip(terms) {
            *slot = if insert { self.intern(term)? } else {
                match self.lookup(&term)? { Some(id) => id, None => return Ok(None) }
            };
        }
        Ok(Some(spo))
    }
    fn graph_id(&self, g: &NamedNode) -> Result<Option<TermId>, StorageError> {
        Ok(self.lookup(&g.clone().into())?.filter(|id| self.named.contains_key(id)))
    }

    /// The default graph, updated in place in the delta (same rules as `apply`).
    /// Whether `spo` is in the default graph as of the delta (ignoring `pending`).
    fn visible_in_delta(&self, spo: [TermId; 3]) -> Result<bool, StorageError> {
        Ok(if self.base.count(Perm::Spo, &spo)? > 0 { self.d.dels[0].binary_search(&spo).is_err() } else { self.d.adds[0].binary_search(&spo).is_ok() })
    }
    fn default_change(&mut self, c: Change, t: &Triple) -> Result<(), StorageError> {
        let Some(spo) = self.ids(t, c == Change::Insert)? else { return Ok(()) };
        let visible = match self.pending.get(&spo) { Some(&v) => v, None => self.visible_in_delta(spo)? };
        if visible == (c == Change::Insert) { return Ok(()) }
        self.changed += 1;
        self.touched.entry(spo).or_insert(visible);
        self.pending.insert(spo, c == Change::Insert);
        Ok(())
    }
    /// Brings the delta up to date with `pending`: per order, one merge of the sorted additions and deletions with
    /// the rows that leave them (same rules as `apply`: additions are absent from the base, deletions present in it).
    fn flush_default(&mut self) -> Result<(), StorageError> {
        if self.pending.is_empty() { return Ok(()) }
        let mut add_in: Vec<[TermId; 3]> = Vec::new();
        let mut add_out: Vec<[TermId; 3]> = Vec::new();
        let mut del_in: Vec<[TermId; 3]> = Vec::new();
        let mut del_out: Vec<[TermId; 3]> = Vec::new();
        for (spo, visible) in std::mem::take(&mut self.pending) {
            if self.visible_in_delta(spo)? == visible { continue } // flipped back to where the delta already is
            match (visible, self.base.count(Perm::Spo, &spo)? > 0) {
                (true, true) => del_out.push(spo),
                (true, false) => add_in.push(spo),
                (false, true) => del_in.push(spo),
                (false, false) => add_out.push(spo),
            }
        }
        for p in Perm::ALL {
            let order = |v: &[[TermId; 3]]| { let mut k: Vec<[TermId; 3]> = v.iter().map(|&r| key(p, r)).collect(); k.sort_unstable(); k };
            let merged = |old: &[[TermId; 3]], add: Vec<[TermId; 3]>, remove: Vec<[TermId; 3]>| -> Vec<[TermId; 3]> {
                let mut out = Vec::with_capacity(old.len() + add.len());
                let (mut a, mut r) = (add.into_iter().peekable(), remove.into_iter().peekable());
                for &row in old {
                    while a.peek().is_some_and(|&x| x < row) { out.push(a.next().unwrap()) }
                    while r.peek().is_some_and(|&x| x < row) { r.next(); }
                    if r.peek() == Some(&row) { r.next(); } else { out.push(row) }
                }
                out.extend(a);
                out
            };
            let i = slot(p);
            self.d.adds[i] = merged(&self.d.adds[i], order(&add_in), order(&add_out));
            self.d.dels[i] = merged(&self.d.dels[i], order(&del_in), order(&del_out));
        }
        Ok(())
    }
    /// Empties the default graph: every base row becomes a deletion, and the additions go.
    fn clear_default(&mut self) -> Result<(), StorageError> {
        self.flush_default()?; // the count below reads the delta
        let visible = self.base.count(Perm::Spo, &[])? as usize - self.d.dels[0].len() + self.d.adds[0].len();
        self.changed += visible;
        self.cleared = true;
        for p in Perm::ALL {
            let mut rows = Vec::new();
            for b in self.base.scan(p, &[]) {
                let b = b?;
                rows.extend((0..b.len()).map(|i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]]));
            }
            self.d.dels[slot(p)] = rows;
            self.d.adds[slot(p)].clear();
        }
        Ok(())
    }
    fn named_change(&mut self, g: &NamedNode, c: Change, t: &Triple) -> Result<(), StorageError> {
        let insert = c == Change::Insert;
        let gid = if insert { self.intern(g.clone().into())? } else {
            match self.graph_id(g)? { Some(id) => id, None => return Ok(()) }
        };
        let Some(spo) = self.ids(t, insert)? else { return Ok(()) };
        // Inserting into an absent graph adds it to the catalog.
        let rows = self.named.entry(gid).or_insert_with(|| Pending::Owned(BTreeSet::new())).owned();
        if if insert { rows.insert(spo) } else { rows.remove(&spo) } { self.changed += 1 }
        Ok(())
    }
    fn create(&mut self, g: &NamedNode, silent: bool) -> Result<(), StorageError> {
        if self.graph_id(g)?.is_some() {
            return if silent { Ok(()) } else { Err(GraphStateError::AlreadyExists(g.clone()).into_io()) };
        }
        let gid = self.intern(g.clone().into())?;
        self.named.insert(gid, Pending::Owned(BTreeSet::new()));
        Ok(())
    }
    /// `DROP` (`remove`) or `CLEAR` of `target`.
    fn empty(&mut self, target: &GraphScope, silent: bool, remove: bool) -> Result<(), StorageError> {
        match target {
            GraphScope::Graph(g) => {
                let Some(gid) = self.graph_id(g)? else {
                    return if silent { Ok(()) } else { Err(GraphStateError::NotFound(g.clone()).into_io()) };
                };
                self.empty_named(gid, remove);
            }
            GraphScope::Default => self.clear_default()?,
            GraphScope::Named => for gid in self.named.keys().copied().collect::<Vec<_>>() { self.empty_named(gid, remove) },
            GraphScope::All => {
                self.clear_default()?;
                for gid in self.named.keys().copied().collect::<Vec<_>>() { self.empty_named(gid, remove) }
            }
        }
        Ok(())
    }
    fn empty_named(&mut self, gid: TermId, remove: bool) {
        self.changed += self.named.get(&gid).map_or(0, Pending::len);
        if remove { self.named.remove(&gid); } else { self.named.insert(gid, Pending::Owned(BTreeSet::new())); }
    }
}

impl Overlay {
    /// This snapshot with the dataset update `changes` applied in order, in memory and without a WAL, plus the number
    /// of triples that changed (in any graph). All or nothing: a refused step (see `GraphStateError`) leaves `self`
    /// as the only state.
    pub fn with_dataset_changes(&self, changes: &[DatasetChange]) -> std::io::Result<(Overlay, usize)> {
        let (next, changed, _) = self.apply_dataset(changes, false)?;
        Ok((next, changed))
    }

    /// `with_dataset_changes`, plus (with `record`) the update's net effect as a WAL record: graph creations, then
    /// triple insertions and deletions in any graph, then graph removals. Re-applying a record (or a sequence of them)
    /// to a state that already contains it changes nothing, which recovery after compaction relies on.
    pub(super) fn apply_dataset(&self, changes: &[DatasetChange], record: bool) -> std::io::Result<(Overlay, usize, Vec<DatasetChange>)> {
        let mut w = Work {
            base: &self.base,
            d: (*self.delta).clone(),
            named: self.named.0.iter().map(|(&g, v)| (g, Pending::Shared(v.rows.clone()))).collect(),
            changed: 0,
            touched: HashMap::new(),
            cleared: false,
            pending: HashMap::new(),
        };
        for change in changes {
            match change {
                DatasetChange::Quad(GraphTarget::Default, c, t) => w.default_change(*c, t)?,
                DatasetChange::Quad(GraphTarget::Named(g), c, t) => w.named_change(g, *c, t)?,
                DatasetChange::Create { graph, silent } => w.create(graph, *silent)?,
                DatasetChange::Drop { target, silent } => w.empty(target, *silent, true)?,
                DatasetChange::Clear { target, silent } => w.empty(target, *silent, false)?,
            }
        }
        w.flush_default()?;
        let (touched, cleared) = (std::mem::take(&mut w.touched), w.cleared);
        let delta = Arc::new(w.d);
        let named: BTreeMap<TermId, NamedGraph> = w.named.into_iter().map(|(g, p)| {
            let rows = match p { Pending::Shared(r) => r, Pending::Owned(s) => Arc::new(Rows::from_spo(&s)) };
            (g, NamedGraph { base: self.base.clone(), delta: delta.clone(), rows })
        }).collect();
        let next = Overlay { base: self.base.clone(), delta, named: Arc::new(Catalog(named)) };
        let rec = if record { self.net_record(&next, touched, cleared)? } else { Vec::new() };
        Ok((next, w.changed, rec))
    }

    /// The net difference from `self` to `next` (see `apply_dataset`). Terms are resolved in `next`, whose dictionary
    /// extends this one.
    fn net_record(&self, next: &Overlay, touched: HashMap<[TermId; 3], bool>, cleared: bool) -> std::io::Result<Vec<DatasetChange>> {
        let triple = |r: [TermId; 3]| -> std::io::Result<Triple> {
            let invalid = |what| StorageError::new(std::io::ErrorKind::InvalidData, what);
            let subject: oxrdf::NamedOrBlankNode = next.resolve(r[0])?.try_into().map_err(|_| invalid("invalid RDF subject"))?;
            let predicate: NamedNode = next.resolve(r[1])?.try_into().map_err(|_| invalid("invalid RDF predicate"))?;
            Ok(Triple::new(subject, predicate, next.resolve(r[2])?))
        };
        let graph_name = |g: TermId| -> std::io::Result<NamedNode> {
            match next.resolve(g)? {
                Term::NamedNode(n) => Ok(n),
                _ => Err(StorageError::new(std::io::ErrorKind::InvalidData, "graph name is not an IRI")),
            }
        };
        let (mut creates, mut quads, mut drops) = (Vec::new(), Vec::new(), Vec::new());
        // Default graph.
        let mut flips: Vec<([TermId; 3], Change)> = Vec::new();
        if cleared {
            let all = |o: &Overlay| -> std::io::Result<Vec<[TermId; 3]>> {
                let mut v = Vec::new();
                for b in o.scan(Perm::Spo, &[]) {
                    let b = b?;
                    v.extend((0..b.len()).map(|i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]]));
                }
                Ok(v)
            };
            let (before, after) = (all(self)?, all(next)?);
            diff(&before, &after, &mut |r, c| flips.push((r, c)));
        } else {
            for (spo, was) in touched {
                if next.contains(spo)? != was { flips.push((spo, if was { Change::Delete } else { Change::Insert })) }
            }
            flips.sort_unstable_by_key(|&(r, _)| r);
        }
        for (r, c) in flips { quads.push(DatasetChange::Quad(GraphTarget::Default, c, triple(r)?)) }
        // Named graphs.
        let gids: BTreeSet<TermId> = self.named.0.keys().chain(next.named.0.keys()).copied().collect();
        for g in gids {
            match (self.named.0.get(&g), next.named.0.get(&g)) {
                (Some(_), None) => drops.push(DatasetChange::Drop { target: GraphScope::Graph(graph_name(g)?), silent: true }),
                (old, Some(new)) => {
                    if old.is_some_and(|old| Arc::ptr_eq(&old.rows, &new.rows)) { continue }
                    let name = graph_name(g)?;
                    if old.is_none() { creates.push(DatasetChange::Create { graph: name.clone(), silent: true }) }
                    let before = old.map_or(&[][..], |o| o.rows(Perm::Spo));
                    let mut err = None;
                    diff(before, new.rows(Perm::Spo), &mut |r, c| {
                        if err.is_some() { return }
                        match triple(r) {
                            Ok(t) => quads.push(DatasetChange::Quad(GraphTarget::Named(name.clone()), c, t)),
                            Err(e) => err = Some(e),
                        }
                    });
                    if let Some(e) = err { return Err(e) }
                }
                (None, None) => {}
            }
        }
        creates.extend(quads);
        creates.extend(drops);
        Ok(creates)
    }

    /// The named graphs as dataset changes (a `Create` per graph, then its triples), for carrying them onto a new
    /// base: the new base has its own IDs, so the graphs are re-interned there.
    pub fn named_graph_changes(&self) -> std::io::Result<Vec<DatasetChange>> {
        let mut out = Vec::new();
        for (&gid, view) in &self.named.0 {
            let Term::NamedNode(graph) = self.resolve(gid)? else {
                return Err(StorageError::new(std::io::ErrorKind::InvalidData, "graph name is not an IRI"));
            };
            out.push(DatasetChange::Create { graph: graph.clone(), silent: false });
            for r in view.rows(Perm::Spo) {
                let invalid = |what| StorageError::new(std::io::ErrorKind::InvalidData, what);
                let subject: oxrdf::NamedOrBlankNode = self.resolve(r[0])?.try_into().map_err(|_| invalid("invalid RDF subject"))?;
                let predicate: NamedNode = self.resolve(r[1])?.try_into().map_err(|_| invalid("invalid RDF predicate"))?;
                let triple = Triple::new(subject, predicate, self.resolve(r[2])?);
                out.push(DatasetChange::Quad(GraphTarget::Named(graph.clone()), Change::Insert, triple));
            }
        }
        Ok(out)
    }

    /// Whether this snapshot has any named graph (empty ones included).
    pub fn has_named_graphs(&self) -> bool { !self.named.is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MemorySnapshot;
    use oxrdf::{BlankNode, Literal};
    use DatasetChange::{Clear, Create, Drop, Quad};

    fn iri(s: &str) -> NamedNode { NamedNode::new(format!("http://ex/{s}")).unwrap() }
    fn t(s: &str, o: &str) -> Triple { Triple::new(iri(s), iri("p"), Literal::new_simple_literal(o)) }
    fn ins(g: &str, tr: Triple) -> DatasetChange { Quad(GraphTarget::Named(iri(g)), Change::Insert, tr) }
    fn del(g: &str, tr: Triple) -> DatasetChange { Quad(GraphTarget::Named(iri(g)), Change::Delete, tr) }
    fn ins_default(tr: Triple) -> DatasetChange { Quad(GraphTarget::Default, Change::Insert, tr) }
    fn graph(g: &str) -> GraphScope { GraphScope::Graph(iri(g)) }

    fn empty() -> Overlay { Overlay::from_base(Arc::new(MemorySnapshot::from_triples([]))) }
    fn with_base(triples: Vec<Triple>) -> Overlay { Overlay::from_base(Arc::new(MemorySnapshot::from_triples(triples))) }
    fn apply(o: &Overlay, changes: &[DatasetChange]) -> (Overlay, usize) { o.with_dataset_changes(changes).unwrap() }
    fn refused(o: &Overlay, changes: &[DatasetChange]) -> GraphStateError {
        let e = o.with_dataset_changes(changes).err().expect("update must be refused");
        e.get_ref().and_then(|e| e.downcast_ref::<GraphStateError>()).expect("typed graph error").clone()
    }

    /// Every named graph by IRI (sorted by IRI), with its triples as N-Triples strings (sorted).
    fn graphs(o: &Overlay) -> Vec<(String, Vec<String>)> {
        let ids = o.graph_names().unwrap();
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "graph_names is in ascending ID order");
        let mut out: Vec<(String, Vec<String>)> = ids.into_iter().map(|g| {
            let view = o.graph(g).unwrap().unwrap();
            let mut rows = Vec::new();
            for b in view.scan(Perm::Spo, &[]) {
                let b = b.unwrap();
                let r = |id: TermId| view.resolve(id).unwrap().to_string();
                for ((&s, &p), &x) in b.cols[0].iter().zip(&b.cols[1]).zip(&b.cols[2]) {
                    rows.push(format!("{} {} {}", r(s), r(p), r(x)));
                }
            }
            rows.sort();
            (o.resolve(g).unwrap().to_string(), rows)
        }).collect();
        out.sort();
        out
    }
    fn names(o: &Overlay) -> Vec<String> { graphs(o).into_iter().map(|(g, _)| g).collect() }

    #[test]
    fn create_insert_clear_drop_follow_the_catalog_rules() {
        let (o, n) = apply(&empty(), &[Create { graph: iri("g3"), silent: false }, ins("g1", t("a", "1")), ins("g1", t("b", "1"))]);
        assert_eq!(n, 2);
        assert_eq!(names(&o), ["<http://ex/g1>", "<http://ex/g3>"], "insert auto-creates; empty graphs are listed");
        assert_eq!(refused(&o, &[Create { graph: iri("g1"), silent: false }]), GraphStateError::AlreadyExists(iri("g1")));
        assert_eq!(apply(&o, &[Create { graph: iri("g1"), silent: true }]).1, 0);
        // Deleting the last triples keeps the graph; CLEAR keeps it; DROP removes it.
        let (o2, n) = apply(&o, &[del("g1", t("a", "1")), del("g1", t("b", "1"))]);
        assert_eq!((n, names(&o2).len()), (2, 2));
        let (o3, n) = apply(&o, &[Clear { target: graph("g1"), silent: false }]);
        assert_eq!((n, graphs(&o3)[0].1.len(), names(&o3).len()), (2, 0, 2));
        let (o4, n) = apply(&o, &[Drop { target: graph("g1"), silent: false }]);
        assert_eq!((n, names(&o4)), (2, vec!["<http://ex/g3>".to_string()]));
        // Absent graphs: an error without SILENT, nothing with it. A deletion from an absent graph is a no-op.
        for op in [Drop { target: graph("nope"), silent: false }, Clear { target: graph("nope"), silent: false }] {
            assert_eq!(refused(&o, &[op]), GraphStateError::NotFound(iri("nope")));
        }
        for op in [Drop { target: graph("nope"), silent: true }, Clear { target: graph("nope"), silent: true }, del("nope", t("a", "1"))] {
            let (same, n) = apply(&o, &[op]);
            assert_eq!((n, graphs(&same)), (0, graphs(&o)));
        }
        // The original overlay is never changed.
        assert_eq!(graphs(&o)[0].1.len(), 2);
    }

    #[test]
    fn refused_update_changes_nothing_and_error_kinds_are_typed() {
        let o = apply(&empty(), &[ins("g1", t("a", "1"))]).0;
        let e = o.with_dataset_changes(&[ins("g2", t("new", "x")), ins_default(t("d", "x")), Create { graph: iri("g1"), silent: false }]).err().unwrap();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        let e = o.with_dataset_changes(&[Drop { target: graph("zz"), silent: false }]).err().unwrap();
        assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(names(&o), ["<http://ex/g1>"]);
        assert_eq!(o.count(Perm::Spo, &[]).unwrap(), 0);
        assert!(o.lookup(&iri("new").into()).unwrap().is_none(), "terms of a refused update are not interned");
    }

    #[test]
    fn steps_apply_in_order() {
        let (o, _) = apply(&empty(), &[ins("g", t("a", "1")), Clear { target: graph("g"), silent: false }, ins("g", t("b", "2"))]);
        assert_eq!(graphs(&o), [("<http://ex/g>".to_string(), vec!["<http://ex/b> <http://ex/p> \"2\"".to_string()])]);
        // Creating a graph that an earlier step of the same update created is refused (the check sees that state).
        assert_eq!(refused(&empty(), &[ins("g", t("a", "1")), Create { graph: iri("g"), silent: false }]), GraphStateError::AlreadyExists(iri("g")));
        let (o, n) = apply(&empty(), &[ins("g", t("a", "1")), Drop { target: graph("g"), silent: false }, Create { graph: iri("g"), silent: false }]);
        assert_eq!((n, graphs(&o)), (2, vec![("<http://ex/g>".to_string(), vec![])]));
    }

    #[test]
    fn default_scope_empties_but_keeps_the_default_graph_and_named_scope_spares_it() {
        let o = with_base(vec![t("base", "d")]);
        let (o, _) = apply(&o, &[ins_default(t("delta", "d")), ins("g1", t("a", "1")), Create { graph: iri("g2"), silent: false }]);
        assert_eq!(o.count(Perm::Spo, &[]).unwrap(), 2);
        for remove in [false, true] {
            let op = |target| if remove { Drop { target, silent: false } } else { Clear { target, silent: false } };
            let (d, n) = apply(&o, &[op(GraphScope::Default)]);
            assert_eq!((n, d.count(Perm::Spo, &[]).unwrap(), graphs(&d)), (2, 0, graphs(&o)));
            assert!(d.scan(Perm::Pos, &[]).next().is_none());
            let (nm, n) = apply(&o, &[op(GraphScope::Named)]);
            assert_eq!((n, nm.count(Perm::Spo, &[]).unwrap()), (1, 2));
            assert_eq!(names(&nm).len(), if remove { 0 } else { 2 });
            let (all, n) = apply(&o, &[op(GraphScope::All)]);
            assert_eq!((n, all.count(Perm::Spo, &[]).unwrap(), names(&all).len()), (3, 0, if remove { 0 } else { 2 }));
        }
        // The default graph can be refilled after it was emptied, including a base triple.
        let (cleared, _) = apply(&o, &[Clear { target: GraphScope::Default, silent: false }]);
        let (refilled, n) = apply(&cleared, &[ins_default(t("base", "d"))]);
        assert_eq!((n, refilled.count(Perm::Spo, &[]).unwrap()), (1, 1));
    }

    #[test]
    fn one_dictionary_across_graphs_and_default() {
        let o = with_base(vec![t("a", "d")]);
        let (o, _) = apply(&o, &[ins("g1", t("a", "1")), ins("g2", t("a", "1")), ins("g2", t("fresh", "2"))]);
        let a = o.lookup(&iri("a").into()).unwrap().unwrap();
        let fresh = o.lookup(&iri("fresh").into()).unwrap().unwrap();
        for g in o.graph_names().unwrap() {
            let view = o.graph(g).unwrap().unwrap();
            assert_eq!(view.lookup(&iri("a").into()).unwrap(), Some(a));
            assert_eq!(view.resolve(fresh).unwrap(), Term::from(iri("fresh")));
        }
        // A later default-graph-only update rebinds the graph views to the newer dictionary.
        let (o, _) = o.with_changes(&[(Change::Insert, t("later", "3"))]).unwrap();
        let later = o.lookup(&iri("later").into()).unwrap().unwrap();
        let g1 = o.graph(o.lookup(&iri("g1").into()).unwrap().unwrap()).unwrap().unwrap();
        assert_eq!(g1.resolve(later).unwrap(), Term::from(iri("later")));
        assert_eq!(graphs(&o).len(), 2);
    }

    /// A graph view answers `scan`, `count`, `seek` and `matching_rows` exactly as a brute-force row model.
    #[test]
    fn graph_view_matches_a_row_model_for_every_order_and_prefix() {
        let mut changes = Vec::new();
        let mut model = Vec::new();
        for i in 0..40u32 {
            let names = [format!("s{}", i % 7), format!("p{}", i % 3), format!("o{}", i % 5)];
            changes.push(ins("g", Triple::new(iri(&names[0]), iri(&names[1]), iri(&names[2]))));
            model.push(names);
        }
        changes.push(ins_default(t("noise", "x")));
        changes.push(ins("other", t("noise", "y")));
        let (o, _) = apply(&with_base(vec![t("s1", "base")]), &changes);
        let g = o.graph(o.lookup(&iri("g").into()).unwrap().unwrap()).unwrap().unwrap();
        let id = |n: &str| o.lookup(&iri(n).into()).unwrap().unwrap();
        let mut spo: Vec<[TermId; 3]> = model.iter().map(|[s, p, x]| [id(s), id(p), id(x)]).collect();
        spo.sort();
        spo.dedup();
        for p in Perm::ALL {
            let mut rows: Vec<[TermId; 3]> = spo.iter().map(|&r| key(p, r)).collect();
            rows.sort();
            let mut prefixes: Vec<Vec<TermId>> = vec![vec![]];
            for r in &rows { for k in 1..=3 { prefixes.push(r[..k].to_vec()) } }
            prefixes.push(vec![TermId(1)]);
            for prefix in &prefixes {
                let want: Vec<[TermId; 3]> = rows.iter().filter(|r| r[..prefix.len()] == prefix[..]).copied().collect();
                let mut got = Vec::new();
                for b in g.scan(p, prefix) { let b = b.unwrap(); got.extend((0..b.len()).map(|i| [b.cols[0][i], b.cols[1][i], b.cols[2][i]])) }
                assert_eq!(got, want, "{p:?} {prefix:?}");
                assert_eq!(g.count(p, prefix).unwrap(), want.len() as u64);
                assert_eq!(g.matching_rows(p, prefix).unwrap(), &want[..]);
                if prefix.len() < 3 {
                    for target in want.iter().map(|r| r[prefix.len()]).chain([TermId(0), TermId(u64::MAX)]) {
                        let expect = want.iter().map(|r| r[prefix.len()]).filter(|&v| v >= target).min();
                        assert_eq!(g.seek(p, prefix, target).unwrap(), expect);
                    }
                }
            }
        }
    }

    /// Random default-graph inserts, deletes and clears over a base, in one update and split over several: the
    /// state, the changed-triple count and every order agree with a plain set model.
    #[test]
    fn batched_default_changes_match_a_set_model() {
        let mut rng = 0x853c49e6748fea9bu64;
        let mut next = move || { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; rng };
        let tr = |i: u64| t(&format!("s{}", i % 13), &format!("{}", i % 7));
        let base: Vec<Triple> = (0..40).map(|i| tr(i * 3)).collect();
        for round in 0..30 {
            let o = with_base(base.clone());
            let mut model: BTreeSet<String> = base.iter().map(|t| t.to_string()).collect();
            let (mut flips, mut steps) = (0usize, Vec::new());
            for _ in 0..200 {
                let r = next();
                if r % 50 == 0 {
                    flips += model.len();
                    model.clear();
                    steps.push(Clear { target: GraphScope::Default, silent: false });
                    continue;
                }
                let triple = tr(r >> 8);
                let insert = r & 1 == 0;
                let changed = if insert { model.insert(triple.to_string()) } else { model.remove(&triple.to_string()) };
                if changed { flips += 1 }
                steps.push(Quad(GraphTarget::Default, if insert { Change::Insert } else { Change::Delete }, triple));
            }
            let (one, n) = apply(&o, &steps);
            assert_eq!(n, flips, "round {round}: changed-triple count");
            // The same steps in several updates end in the same state.
            let mut split = o.with_dataset_changes(&[]).unwrap().0;
            for chunk in steps.chunks(1 + round % 17) { split = apply(&split, chunk).0 }
            for got in [&one, &split] {
                let mut rows = Vec::new();
                for b in got.scan(Perm::Spo, &[]) {
                    let b = b.unwrap();
                    let r = |id: TermId| got.resolve(id).unwrap().to_string();
                    for ((&s, &p), &x) in b.cols[0].iter().zip(&b.cols[1]).zip(&b.cols[2]) {
                        rows.push(format!("{} {} {}", r(s), r(p), r(x)));
                    }
                }
                rows.sort();
                assert_eq!(rows, model.iter().cloned().collect::<Vec<_>>(), "round {round}");
                for p in Perm::ALL { assert_eq!(got.count(p, &[]).unwrap(), model.len() as u64, "round {round} {p:?}") }
            }
        }
    }

    /// Descriptive timing for a large single update (run with `--ignored --nocapture`).
    #[test]
    #[ignore]
    fn large_default_update_time() {
        for n in [100_000u64, 1_000_000] {
            let steps: Vec<DatasetChange> = (0..n).map(|i| ins_default(Triple::new(iri(&format!("s{}", i * 7919 % n)), iri("p"), Literal::new_simple_literal(i.to_string())))).collect();
            let start = std::time::Instant::now();
            let (o, changed) = apply(&empty(), &steps);
            println!("{n} default-graph quads in one update: {:.2} s ({changed} changed, {} rows)", start.elapsed().as_secs_f64(), o.count(Perm::Spo, &[]).unwrap());
            // Reference: the same triples as a bulk build (the `.nt`-into-empty-memory-store path).
            let triples: Vec<Triple> = steps.iter().map(|c| match c { Quad(_, _, t) => t.clone(), _ => unreachable!() }).collect();
            let start = std::time::Instant::now();
            let bulk = MemorySnapshot::from_triples(triples);
            println!("{n} triples as a bulk build: {:.2} s ({} rows)", start.elapsed().as_secs_f64(), bulk.count(Perm::Spo, &[]).unwrap());
        }
    }

    #[test]
    fn blank_node_sharing_is_exact() {
        let x = BlankNode::new("x").unwrap();
        let bx = |o: &str| Triple::new(x.clone(), iri("p"), Literal::new_simple_literal(o));
        let (o, _) = apply(&empty(), &[ins("g1", bx("1")), ins("g2", bx("2")), ins("g3", t("a", "3")),
                                       ins("g4", Triple::new(iri("a"), iri("p"), x.clone()))]);
        let g = |n: &str| o.lookup(&iri(n).into()).unwrap().unwrap();
        assert_eq!(o.shares_blank_nodes(&[g("g1"), g("g2")]).unwrap(), Some(true));
        assert_eq!(o.shares_blank_nodes(&[g("g1"), g("g4")]).unwrap(), Some(true), "object position counts");
        assert_eq!(o.shares_blank_nodes(&[g("g1"), g("g3")]).unwrap(), Some(false));
        assert_eq!(o.shares_blank_nodes(&[g("g1"), g("g1")]).unwrap(), Some(false), "one graph listed twice");
    }

    #[test]
    fn named_graphs_carry_over_onto_a_new_base() {
        let x = BlankNode::new("x").unwrap();
        let (o, _) = apply(&with_base(vec![t("a", "d")]), &[
            ins("g1", t("a", "1")), ins("g1", Triple::new(x.clone(), iri("p"), x.clone())), Create { graph: iri("empty"), silent: false }]);
        let fresh = Overlay::from_base(Arc::new(MemorySnapshot::from_triples([t("a", "d"), t("unrelated", "u")])));
        let (moved, _) = fresh.with_dataset_changes(&o.named_graph_changes().unwrap()).unwrap();
        assert_eq!(graphs(&moved), graphs(&o));
        assert!(!empty().has_named_graphs() && moved.has_named_graphs());
    }
}
