//! Query executor (owner: Kaiming-He, task #4).
//!
//! Solutions are full-width rows of `TermId` over every variable of the query; `UNBOUND` (0) marks an unbound
//! slot. Storage guarantees real IDs are never 0. Terms computed by expressions (BIND, aggregates) that are not
//! in the store get per-query scratch IDs, so ID equality stays term equality throughout.
// Paths used throughout this crate (`crate::model`, `crate::sparql::...`, `crate::opt::...`).
pub(crate) use triplelight_core::{model, storage};
pub(crate) use triplelight_plan as opt;
pub(crate) use triplelight_sparql as sparql;
pub(crate) mod store { pub use triplelight_core::model::Change; }

/// Runs `$body` with `$g` bound to the active graph of `$ctx`: the concrete snapshot (monomorphic fast path) or the
/// dataset graph currently selected by GRAPH / FROM (trait object).
macro_rules! on_data {
    ($ctx:expr, |$g:ident| $body:expr) => {
        match $ctx.data.get() { Some($g) => $body, None => { let $g = $ctx.snap; $body } }
    };
}

mod agg;
mod bgp;
mod count;
mod dataset;
mod path;
mod expr;
mod support;
use bgp::Slot;
pub mod results;

use crate::model::{Perm, TermId};
use crate::storage::{DatasetView, Snapshot};
use crate::opt::stats::Stats;
use oxrdf::{BlankNode, Literal, Term, Variable};
use crate::sparql::{AggregateExpression, AggregateFunction, Expression, Function, GraphPattern, OrderExpression};
use crate::sparql::{GroundTerm, NamedNodePattern, TermPattern, TriplePattern};
use crate::sparql::Query;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

pub const UNBOUND: TermId = TermId(0);
const SCRATCH: u64 = 0xe << 60;

pub type Row = Vec<TermId>;

#[derive(Debug)]
pub enum QueryResult {
    Solutions { vars: Vec<Variable>, rows: Vec<Vec<Option<Term>>> },
    /// Undecoded solutions (with `Options::raw_ids`): serialize with `results::serialize_ids`.
    Ids(IdSolutions),
    Boolean(bool),
    Graph(Vec<oxrdf::Triple>),
}

/// SELECT results as term IDs: `ids` holds `vars.len()` IDs per row (`UNBOUND` = unbound). Computed terms that the
/// store doesn't know carry scratch IDs, resolved in `scratch`.
#[derive(Debug)]
pub struct IdSolutions {
    pub vars: Vec<Variable>,
    pub ids: Vec<TermId>,
    pub scratch: HashMap<TermId, Term>,
    /// Number of solutions. Needed because a query with no projected variables (`SELECT * { <s> <p> <o> }`) still
    /// has one empty solution per match, which `ids` alone cannot count.
    pub rows: usize,
}

impl IdSolutions {
    pub fn width(&self) -> usize { self.vars.len() }
    pub fn len(&self) -> usize { self.rows }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
    /// Decodes to owned terms (in parallel; decoding dominates large results).
    pub fn decode<D: crate::storage::Dictionary + Sync + ?Sized>(self, dict: &D) -> QueryResult {
        use rayon::prelude::*;
        let w = self.width();
        let rows = if w == 0 { vec![Vec::new(); self.rows] } else {
            self.ids.par_chunks(w).with_min_len(4096).map(|r| r.iter().map(|&id| {
                if id == UNBOUND { None } else if let Some(t) = self.scratch.get(&id) { Some(t.clone()) } else { Some(dict.resolve(id).unwrap_or_else(|e| storage_failure(e))) }
            }).collect()).collect()
        };
        QueryResult::Solutions { vars: self.vars, rows }
    }
}

#[derive(Debug)]
pub struct QueryError { pub kind: QueryErrorKind, pub message: String }

/// What failed: the caller's query (HTTP 400), a feature we don't implement (501), or the store itself (500).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryErrorKind { Syntax, Unsupported, Storage }

impl QueryError {
    pub fn new(kind: QueryErrorKind, message: impl Into<String>) -> Self { QueryError { kind, message: message.into() } }
}
impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.message) }
}
impl std::error::Error for QueryError {}

pub fn query<S: Snapshot>(snap: &S, text: &str) -> Result<QueryResult, QueryError> {
    query_with(snap, text, &Options::default())
}

#[derive(Clone, Copy)]
pub struct Options<'a> {
    /// Statistics built from this same snapshot: BGP join order then comes from the cost-based planner (task #3);
    /// without, from a greedy exact-count heuristic.
    pub stats: Option<&'a Stats>,
    /// Use the worst-case-optimal leapfrog join for cyclic top-level BGPs.
    pub wcoj: bool,
    /// Return SELECT results undecoded (`QueryResult::Ids`).
    pub raw_ids: bool,
    /// Disable the ID hash-aggregation fast path for GROUP BY (`try_fast_group`), so groups that would take it use
    /// the general per-group implementation. Index count pushdown (`try_count_group`) still runs first, exactly as
    /// with the former process-global `TRIPLELIGHT_LEGACY_GROUP` environment variable this replaces. Answers are
    /// identical; this exists for differential verification (tests, release checks, benchmark controls).
    pub general_group: bool,
}
impl Default for Options<'_> { fn default() -> Self { Options { stats: None, wcoj: true, raw_ids: false, general_group: false } } }

pub fn query_with<S: Snapshot>(snap: &S, text: &str, opts: &Options<'_>) -> Result<QueryResult, QueryError> {
    catching_storage(|| run(snap, None, text, opts))
}

/// Storage reads inside evaluation unwind with `StorageFailure`; turn that into a `Storage` error.
fn catching_storage<T>(f: impl FnOnce() -> Result<T, QueryError>) -> Result<T, QueryError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => match p.downcast::<StorageFailure>() {
            Ok(f) => Err(QueryError::new(QueryErrorKind::Storage, format!("storage error: {}", f.0))),
            Err(p) => std::panic::resume_unwind(p),
        },
    }
}

fn run<S: Snapshot>(snap: &S, dataset: Option<&dyn DatasetView>, text: &str, opts: &Options<'_>) -> Result<QueryResult, QueryError> {
    let q = crate::sparql::parse_query(text, None).map_err(|e| QueryError::new(QueryErrorKind::Syntax, e.to_string()))?;
    run_query(snap, dataset, &q, opts)
}

/// Evaluates `text` against an RDF dataset (task #36): `snap` is the dataset's stored default graph and dictionary,
/// `dataset` its named-graph catalog over the same snapshot. Supports GRAPH, FROM and FROM NAMED with local graphs:
/// - no FROM / FROM NAMED: the default graph is `snap`; GRAPH ranges over every catalog graph (empty ones included);
/// - FROM g1..gn: the default graph is the duplicate-free merge of the listed graphs; FROM NAMED only: it is empty;
/// - FROM NAMED h1..hm: GRAPH ranges over the listed graphs; FROM without FROM NAMED: over none.
///
/// Dataset construction is local-only: a FROM / FROM NAMED source missing from the catalog is refused with an
/// `Unsupported` error (present-but-empty graphs are fine); nothing is loaded remotely. `GRAPH <missing>` without a
/// dataset clause is simply an empty match. The default graph is never a value of a GRAPH variable.
pub fn query_dataset_with<S: Snapshot>(snap: &S, dataset: &dyn DatasetView, text: &str, opts: &Options<'_>) -> Result<QueryResult, QueryError> {
    catching_storage(|| run(snap, Some(dataset), text, opts))
}

/// Graph views and named graphs of the active dataset. Every FROM / FROM NAMED source must be a graph of the catalog.
fn resolve_dataset<'a, S: Snapshot>(snap: &S, ds: &'a dyn DatasetView, spec: Option<&crate::sparql::QueryDataset>) -> Result<(Vec<&'a dyn Snapshot>, Vec<TermId>), QueryError> {
    let storage = |e: crate::storage::StorageError| QueryError::new(QueryErrorKind::Storage, format!("storage error: {e}"));
    let mut from: Vec<&dyn Snapshot> = Vec::new();
    let mut named: Vec<TermId> = Vec::new();
    // Local-only dataset construction: every FROM / FROM NAMED source must be a graph of the catalog (present,
    // possibly empty). A missing source is refused rather than silently omitted or invented as an empty graph.
    let present = |iris: &[oxrdf::NamedNode]| -> Result<Vec<TermId>, QueryError> {
        let mut ids = Vec::new();
        for iri in iris {
            let id = snap.lookup(&iri.clone().into()).map_err(storage)?;
            match id {
                Some(id) if ds.graph(id).map_err(storage)?.is_some() => ids.push(id),
                _ => return Err(QueryError::new(QueryErrorKind::Unsupported, format!(
                    "dataset source <{}> is not a graph of this database (only local graphs can be named in FROM / FROM NAMED)", iri.as_str()))),
            }
        }
        ids.sort_unstable(); ids.dedup();
        Ok(ids)
    };
    match spec {
        None => named = ds.graph_names().map_err(storage)?,
        Some(spec) => {
            let default = present(&spec.default)?;
            if default.len() > 1 && ds.shares_blank_nodes(&default).map_err(storage)? != Some(false) {
                return Err(QueryError::new(QueryErrorKind::Unsupported,
                    "FROM over several graphs that share (or may share) blank nodes is not supported: the RDF merge would standardize them apart".to_string()));
            }
            for id in default { from.push(ds.graph(id).map_err(storage)?.expect("checked present")) }
            if let Some(n) = &spec.named { named = present(n)? }
        }
    }
    Ok((from, named))
}

fn run_query<S: Snapshot>(snap: &S, dataset: Option<&dyn DatasetView>, q: &Query, opts: &Options<'_>) -> Result<QueryResult, QueryError> {
    // Reject what we do not evaluate instead of answering it wrongly (ignored GRAPH, a function that errors per row).
    if let Some(reason) = support::unsupported(q, dataset.is_some()) { return Err(QueryError::new(QueryErrorKind::Unsupported, reason)) }
    let spec = match q { Query::Select { dataset, .. } | Query::Ask { dataset, .. } | Query::Construct { dataset, .. } | Query::Describe { dataset, .. } => dataset };
    // Graph views for the active dataset; they outlive `ctx`, which borrows them.
    let (from, named) = match dataset { Some(ds) => resolve_dataset(snap, ds, spec.as_ref())?, None => (Vec::new(), Vec::new()) };
    let empty = dataset::EmptyGraph { dict: snap as &dyn Snapshot };
    let union = (from.len() > 1).then(|| dataset::UnionGraph { members: from.clone() });
    let mut ctx = Ctx::new(snap);
    ctx.dataset = dataset;
    ctx.named = named;
    if spec.is_some() && dataset.is_some() {
        let default: &dyn Snapshot = match (from.len(), &union) { (0, _) => &empty, (1, _) => from[0], (_, Some(u)) => u, _ => unreachable!() };
        ctx.data.set(Some(default));
    }
    ctx.base = match q { Query::Select { base_iri, .. } | Query::Ask { base_iri, .. } | Query::Construct { base_iri, .. } | Query::Describe { base_iri, .. } => base_iri.clone() };
    ctx.wcoj = opts.wcoj;
    ctx.general_group = opts.general_group;
    let stats = opts.stats;
    ctx.stats = stats.map(|st| (st, snap as &dyn Snapshot));
    match q {
        Query::Select { pattern, .. } => run_select(&mut ctx, snap, pattern, opts),
        Query::Ask { pattern, .. } => run_ask(&mut ctx, pattern),
        Query::Construct { template, pattern, .. } => run_construct(&mut ctx, template, pattern),
        Query::Describe { pattern, .. } => run_describe(&mut ctx, pattern),
    }
}

fn run_select<S: Snapshot>(ctx: &mut Ctx<'_, S>, snap: &S, pattern: &GraphPattern, opts: &Options<'_>) -> Result<QueryResult, QueryError> {
    // The projection sits under any LIMIT/OFFSET and DISTINCT/REDUCED wrappers.
    fn projected(p: &GraphPattern) -> Option<&Vec<Variable>> {
        match p {
            GraphPattern::Project { variables, .. } => Some(variables),
            GraphPattern::Slice { inner, .. } | GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner } => projected(inner),
            _ => None,
        }
    }
    let vars = projected(pattern).cloned().unwrap_or_else(|| ctx.collect_visible(pattern));
    ctx.register_all(pattern);
    let rows = ctx.eval(pattern, vec![ctx.empty_row()]);
    let idx: Vec<usize> = vars.iter().map(|v| ctx.var(v)).collect();
    // Keep results as IDs: projected columns in a flat buffer, plus the few computed (scratch) terms.
    let w = idx.len();
    let mut ids = Vec::with_capacity(rows.len() * w);
    for r in &rows { for &i in &idx { ids.push(r[i]) } }
    let scratch: HashMap<TermId, Term> = ids.iter().filter(|id| id.0 >> 60 == 0xe)
        .map(|&id| (id, ctx.resolve(id))).collect();
    let sol = IdSolutions { vars, ids, scratch, rows: rows.len() };
    if opts.raw_ids { return Ok(QueryResult::Ids(sol)) }
    Ok(sol.decode(snap))
}

fn run_ask<S: Snapshot>(ctx: &mut Ctx<'_, S>, pattern: &GraphPattern) -> Result<QueryResult, QueryError> {
    ctx.register_all(pattern);
    let rows = ctx.eval(&GraphPattern::Slice { inner: Box::new(pattern.clone()), start: 0, length: Some(1) },
                        vec![ctx.empty_row()]);
    Ok(QueryResult::Boolean(!rows.is_empty()))
}

fn run_construct<S: Snapshot>(ctx: &mut Ctx<'_, S>, template: &[TriplePattern], pattern: &GraphPattern) -> Result<QueryResult, QueryError> {
    ctx.register_all(pattern);
    for t in template { ctx.register_triple(t); }
    let rows = ctx.eval(pattern, vec![ctx.empty_row()]);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (n, r) in rows.iter().enumerate() {
        for t in template {
            let inst = |tp: &TermPattern| -> Option<Term> {
                match tp {
                    TermPattern::BlankNode(b) => Some(BlankNode::new_unchecked(format!("{}r{n}", b.as_str())).into()),
                    tp => ctx.term_slot(tp, r).and_then(|id| (id != UNBOUND).then(|| ctx.resolve(id))),
                }
            };
            let (Some(s), Some(o)) = (inst(&t.subject), inst(&t.object)) else { continue };
            let p = match &t.predicate {
                NamedNodePattern::NamedNode(n) => Some(Term::NamedNode(n.clone())),
                NamedNodePattern::Variable(v) => { let id = r[ctx.var(v)]; (id != UNBOUND).then(|| ctx.resolve(id)) }
            };
            let (Some(Term::NamedNode(p)), Ok(s)) = (p, oxrdf::NamedOrBlankNode::try_from(s)) else { continue };
            let tr = oxrdf::Triple::new(s, p, o);
            if seen.insert(tr.clone()) { out.push(tr); }
        }
    }
    Ok(QueryResult::Graph(out))
}

fn run_describe<S: Snapshot>(ctx: &mut Ctx<'_, S>, pattern: &GraphPattern) -> Result<QueryResult, QueryError> {
    // SPARQL leaves the description implementation-defined. TripleLight's policy: every triple whose subject is
    // a described resource (an IRI listed in the query, or a value of a projected variable), plus, recursively,
    // the triples of blank nodes reached as objects, so a description never ends in a dangling blank node.
    // Incoming triples are not included.
    ctx.register_all(pattern);
    let rows = ctx.eval(pattern, vec![ctx.empty_row()]);
    let mut queue: Vec<TermId> = Vec::new();
    let mut seen = HashSet::new();
    for r in &rows { for &id in r { if id != UNBOUND && id.0 >> 60 != 0xe && seen.insert(id) { queue.push(id) } } }
    let mut out = Vec::new();
    while let Some(s) = queue.pop() {
        let Ok(subject) = oxrdf::NamedOrBlankNode::try_from(ctx.resolve(s)) else { continue };
        let mut triples = Vec::new();
        ctx.scan_each(Perm::Spo, &[s], |_, p, o| triples.push((p, o)));
        for (p, o) in triples {
            let Term::NamedNode(p) = ctx.resolve(p) else { continue };
            let object = ctx.resolve(o);
            if matches!(object, Term::BlankNode(_)) && seen.insert(o) { queue.push(o) }
            out.push(oxrdf::Triple::new(subject.clone(), p, object));
        }
    }
    Ok(QueryResult::Graph(out))
}


pub(crate) struct Ctx<'a, S: Snapshot + ?Sized> {
    /// Dictionary, and the active graph unless `data` overrides it.
    pub(crate) snap: &'a S,
    /// Active graph when it is not `snap`: a named graph inside GRAPH, or the FROM default graph. `None` keeps the
    /// monomorphic fast path for ordinary default-graph queries.
    data: std::cell::Cell<Option<&'a dyn Snapshot>>,
    /// Dataset of a dataset-aware query (`query_dataset_with`); `None` for plain snapshot queries.
    dataset: Option<&'a dyn DatasetView>,
    /// Named graphs of the active dataset (catalog, or the FROM NAMED graphs present in it), ascending.
    named: Vec<TermId>,
    vars: HashMap<String, usize>,
    scratch: RefCell<(HashMap<Term, TermId>, Vec<Term>)>,
    stats: Option<(&'a Stats, &'a dyn Snapshot)>,
    wcoj: bool,
    /// `Options::general_group`.
    general_group: bool,
    /// LIMIT (+OFFSET) of a Slice directly above the next ORDER BY (through at most a Project): lets ORDER BY keep
    /// only the first `k` rows (stable top-k) instead of sorting everything. Taken by that ORDER BY.
    order_limit: std::cell::Cell<Option<usize>>,
    /// The query's BASE IRI, used by IRI()/URI() to resolve relative strings.
    pub(crate) base: Option<String>,
    /// NOW(): one xsd:dateTime per query execution.
    now: String,
    /// EXISTS / NOT EXISTS results memoized per pattern and per binding of the pattern's variables. Whether a
    /// pattern has a solution compatible with a row depends only on the row's values for the pattern's own
    /// variables, so rows that differ elsewhere (e.g. many offers of one product) share one evaluation.
    /// Keys use a slot id, not the pattern's address: rewrites evaluate temporary patterns whose addresses are
    /// reused, so a slot is only reused when the pattern at that address is structurally the same.
    exists_memo: RefCell<HashMap<(usize, usize, Vec<TermId>), bool>>,
    exists_vars: RefCell<HashMap<usize, (GraphPattern, usize, Option<Vec<usize>>)>>,
    memo_slots: std::cell::Cell<usize>,
    /// Aggregate argument values memoized per expression and per ID of its single variable: the same term (e.g.
    /// one offer's price) recurs in many groups, and evaluating it means a dictionary decode each time.
    agg_arg_memo: RefCell<HashMap<(usize, TermId), Option<Term>>>,
    agg_arg_slots: RefCell<HashMap<usize, (Expression, usize)>>,
}

impl<'a, S: Snapshot + ?Sized> Ctx<'a, S> {
    fn new(snap: &'a S) -> Self {
        Ctx { snap, data: std::cell::Cell::new(None), dataset: None, named: Vec::new(), vars: HashMap::new(), scratch: RefCell::new((HashMap::new(), Vec::new())), stats: None, wcoj: true, general_group: false, order_limit: std::cell::Cell::new(None), base: None, now: expr::now_lexical(), exists_memo: RefCell::new(HashMap::new()), exists_vars: RefCell::new(HashMap::new()), agg_arg_memo: RefCell::new(HashMap::new()), agg_arg_slots: RefCell::new(HashMap::new()), memo_slots: std::cell::Cell::new(0) }
    }
    fn empty_row(&self) -> Row { vec![UNBOUND; self.vars.len()] }
    fn var(&self, v: &Variable) -> usize { self.vars[v.as_str()] }
    fn var_name(&mut self, name: &str) -> usize {
        let n = self.vars.len();
        *self.vars.entry(name.to_string()).or_insert(n)
    }

    // Storage reads are fallible; an error aborts the whole query (caught in `query_with`), never yields "no rows".
    fn count(&self, perm: Perm, prefix: &[TermId]) -> u64 { on_data!(self, |g| g.count(perm, prefix)).unwrap_or_else(|e| storage_failure(e)) }
    /// Identity of the active graph, for memo keys (0 = `snap`).
    fn graph_token(&self) -> usize { self.data.get().map_or(0, |g| g as *const dyn Snapshot as *const () as usize) }
    /// Evaluates `f` with `g` as the active graph; statistics describe `snap` only, so planning falls back to exact counts.
    fn with_graph<T>(&self, g: &'a dyn Snapshot, f: impl FnOnce() -> T) -> T {
        let prev = self.data.replace(Some(g));
        struct Restore<'c, 'x>(&'c std::cell::Cell<Option<&'x dyn Snapshot>>, Option<&'x dyn Snapshot>);
        impl Drop for Restore<'_, '_> { fn drop(&mut self) { self.0.set(self.1) } }
        let _restore = Restore(&self.data, prev);
        f()
    }
    pub(crate) fn resolve(&self, id: TermId) -> Term {
        if id.0 >> 60 == 0xe { return self.scratch.borrow().1[(id.0 & !(0xf << 60)) as usize].clone(); }
        self.snap.resolve(id).unwrap_or_else(|e| storage_failure(e))
    }
    /// Dictionary lookup; a storage failure aborts the query.
    fn lookup(&self, t: &Term) -> Option<TermId> { self.snap.lookup(t).unwrap_or_else(|e| storage_failure(e)) }
    /// ID for any term: the store's ID if present, else a stable scratch ID.
    pub(crate) fn intern(&self, t: &Term) -> TermId {
        // `scratch.0` caches every interned term, stored or not: computed values (e.g. COUNT results, one per
        // group) repeat, and a stored-ID lookup is a dictionary search on disk.
        if let Some(&id) = self.scratch.borrow().0.get(t) { return id; }
        if let Some(id) = self.lookup(t) {
            if self.scratch.borrow().0.len() < 1 << 20 { self.scratch.borrow_mut().0.insert(t.clone(), id); }
            return id;
        }
        let mut s = self.scratch.borrow_mut();
        let id = TermId(SCRATCH | s.1.len() as u64);
        s.0.insert(t.clone(), id);
        s.1.push(t.clone());
        id
    }

    // ---------- variable registration ----------
    fn register_all(&mut self, p: &GraphPattern) {
        let mut names = Vec::new();
        walk_vars(p, &mut names);
        for n in names { self.var_name(&n); }
    }
    fn register_triple(&mut self, t: &TriplePattern) {
        for tp in [&t.subject, &t.object] { if let TermPattern::Variable(v) = tp { self.var_name(v.as_str()); } }
        if let NamedNodePattern::Variable(v) = &t.predicate { self.var_name(v.as_str()); }
    }
    fn collect_visible(&self, p: &GraphPattern) -> Vec<Variable> {
        let mut names = Vec::new();
        walk_vars(p, &mut names);
        names.into_iter().filter(|n| !crate::sparql::is_internal_variable(n)).map(Variable::new_unchecked).collect()
    }
    /// For CONSTRUCT templates: value of a template slot under solution `row`.
    fn term_slot(&self, tp: &TermPattern, row: &Row) -> Option<TermId> {
        match tp {
            TermPattern::Variable(v) => Some(row[self.var(v)]),
            TermPattern::NamedNode(n) => Some(self.intern(&n.clone().into())),
            TermPattern::Literal(l) => Some(self.intern(&l.clone().into())),
            _ => None,
        }
    }

    // ---------- evaluation ----------
    /// Evaluates `p` for each input row (sideways information passing: bound values flow into scans).
    pub(crate) fn eval(&self, p: &GraphPattern, input: Vec<Row>) -> Vec<Row> {
        match p {
            GraphPattern::Bgp { patterns } => self.eval_bgp(patterns, input),
            GraphPattern::Join { left, right } => {
                let l = self.eval(left, input);
                // Passing the left solutions into the right group (sideways) is only equivalent to SPARQL's
                // bottom-up join when no FILTER/BIND in the right group reads a variable that group does not bind
                // itself; otherwise evaluate the right group on its own and join the solution sets.
                if scope_safe(right) { self.eval(right, l) } else { join_rows(l, self.eval(right, vec![self.empty_row()])) }
            }
            GraphPattern::LeftJoin { left, right, expression } => self.eval_left_join(left, right, expression, input),
            GraphPattern::Filter { expr, inner } => self.eval_filter(expr, inner, input),
            GraphPattern::Union { left, right } => {
                let mut l = self.eval(left, input.clone());
                l.extend(self.eval(right, input));
                l
            }
            GraphPattern::Extend { inner, variable, expression } => self.eval_extend(inner, variable, expression, input),
            GraphPattern::Minus { left, right } => self.eval_minus(left, right, input),
            GraphPattern::Values { variables, bindings } => self.eval_values(variables, bindings, input),
            GraphPattern::OrderBy { inner, expression } => self.eval_order_by(inner, expression, input),
            GraphPattern::Project { inner, variables } => {
                let keep: HashSet<usize> = variables.iter().map(|v| self.var(v)).collect();
                // Inner variables are scoped to the subquery: evaluate from an empty row, then join back.
                let inner_rows = self.eval(inner, vec![self.empty_row()]);
                let projected: Vec<Row> = inner_rows.into_iter().map(|mut r| {
                    for (i, x) in r.iter_mut().enumerate() { if !keep.contains(&i) { *x = UNBOUND } }
                    r
                }).collect();
                join_rows(input, projected)
            }
            GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner } => {
                let mut seen = HashSet::new();
                self.eval(inner, input).into_iter().filter(|r| seen.insert(r.clone())).collect()
            }
            GraphPattern::Slice { inner, start, length } => self.eval_slice(inner, start, length, input),
            GraphPattern::Group { inner, variables, aggregates } => self.eval_group(inner, variables, aggregates, input),
            GraphPattern::Graph { name, inner } => self.eval_graph(name, inner, input),
            GraphPattern::Path { subject, path, object } => self.eval_path(subject, path, object, input),
            GraphPattern::Service { .. } => panic!("SERVICE is not supported"),
        }
    }

    fn eval_left_join(&self, left: &GraphPattern, right: &GraphPattern, expression: &Option<Expression>, input: Vec<Row>) -> Vec<Row> {
        let l = self.eval(left, input);
        let mut out = Vec::new();
        // As for Join: the OPTIONAL group sees the left solution only if it is scope-safe. The LeftJoin's own
        // expression is evaluated on the merged solution either way.
        let standalone = (!scope_safe(right)).then(|| self.eval(right, vec![self.empty_row()]));
        for row in l {
            let merged = match &standalone { Some(r) => join_rows(vec![row.clone()], r.clone()), None => self.eval(right, vec![row.clone()]) };
            let ext: Vec<Row> = merged.into_iter().filter(|r| expression.as_ref().is_none_or(|e| self.ebv(e, r))).collect();
            if ext.is_empty() { out.push(row) } else { out.extend(ext) }
        }
        out
    }

    fn eval_filter(&self, expr: &Expression, inner: &GraphPattern, input: Vec<Row>) -> Vec<Row> {
        // Filter pushdown: conjuncts over variables the BGP binds are applied inside it, right after the
        // join step that binds their last variable, so failing rows are never extended further.
        if let GraphPattern::Bgp { patterns } = inner {
            // `FILTER(?a = ?b)` (or sameTerm) where neither variable can be bound to a literal: equality is term
            // identity, so evaluate the BGP with ?b renamed to ?a (a join instead of a cross product) and copy
            // ?a into ?b afterwards. Literals are excluded because `=` on literals is value equality.
            let all = conjuncts(expr);
            let unify = all.iter().enumerate().find_map(|(k, c)| self.unifiable_equality(c, patterns).map(|(a, b)| (k, a, b)))
                // Other conjuncts must not read ?b: it is only filled in after the renamed BGP.
                .filter(|(k, _, b)| all.iter().enumerate().all(|(i, c)| i == *k || !expr_vars(c).iter().any(|n| n == b.as_str())));
            // Only when no incoming row binds either variable: an outer binding of ?b (VALUES, OPTIONAL, a
            // nested evaluation) must still be checked, not overwritten.
            let unify = unify.filter(|(_, a, b)| {
                let (ai, bi) = (self.var(a), self.var(b));
                input.iter().all(|r| r[ai] == UNBOUND && r[bi] == UNBOUND)
            });
            if let Some((k, a, b)) = unify {
                let renamed: Vec<TriplePattern> = patterns.iter().map(|t| rename_var(t, &b, &a)).collect();
                let rest: Vec<&Expression> = all.iter().enumerate().filter(|(i, _)| *i != k).map(|(_, c)| *c).collect();
                let inner2 = GraphPattern::Bgp { patterns: renamed };
                let (ai, bi) = (self.var(&a), self.var(&b));
                let rows = match rest.split_first() {
                    None => self.eval(&inner2, input),
                    Some((first, more)) => {
                        let e = more.iter().fold((*first).clone(), |acc, c| Expression::And(Box::new(acc), Box::new((*c).clone())));
                        self.eval(&GraphPattern::Filter { expr: e, inner: Box::new(inner2) }, input)
                    }
                };
                return rows.into_iter().map(|mut r| { r[bi] = r[ai]; r }).collect();
            }
            let bgp_vars: HashSet<usize> = { let mut v = Vec::new(); walk_vars(inner, &mut v); v.iter().filter_map(|n| self.vars.get(n).copied()).collect() };
            let (early, late): (Vec<&Expression>, Vec<&Expression>) = conjuncts(expr).into_iter().partition(|c| {
                let vs = expr_vars(c);
                !vs.is_empty() && !impure(c) && vs.iter().all(|n| self.vars.get(n).is_some_and(|i| bgp_vars.contains(i)))
            });
            if !early.is_empty() {
                let early: Vec<(&Expression, Vec<usize>)> = early.into_iter().map(|c| (c, expr_vars(c).iter().map(|n| self.vars[n]).collect())).collect();
                let rows = self.eval_bgp_filtered(patterns, input, &early);
                return rows.into_iter().filter(|r| late.iter().all(|c| self.ebv(c, r))).collect();
            }
        }
        let rows = self.eval(inner, input);
        if let Some(out) = self.exists_batch(expr, &rows) { return out }
        rows.into_iter().filter(|r| self.ebv(expr, r)).collect()
    }

    fn eval_extend(&self, inner: &GraphPattern, variable: &Variable, expression: &Expression, input: Vec<Row>) -> Vec<Row> {
        let i = self.var(variable);
        // If an outer solution passed in sideways already binds the target, the BIND result must be
        // compatible with it (the group's solution is joined with the outer one): equal keeps the row, a
        // different value drops it, an expression error leaves the outer value (W3C-style `{ BIND(2 AS ?x) }`).
        self.eval(inner, input).into_iter().filter_map(|mut r| {
            if let Some(t) = self.value(expression, &r) {
                let id = self.intern(&t);
                if r[i] == UNBOUND { r[i] = id } else if r[i] != id { return None }
            }
            Some(r)
        }).collect()
    }

    fn eval_minus(&self, left: &GraphPattern, right: &GraphPattern, input: Vec<Row>) -> Vec<Row> {
        let l = self.eval(left, input);
        let r = self.eval(right, vec![self.empty_row()]);
        l.into_iter().filter(|a| !r.iter().any(|b| {
            let mut shared = false;
            for (x, y) in a.iter().zip(b) {
                if *x != UNBOUND && *y != UNBOUND { if x != y { return false } shared = true }
            }
            shared
        })).collect()
    }

    fn eval_values(&self, variables: &[Variable], bindings: &[Vec<Option<GroundTerm>>], input: Vec<Row>) -> Vec<Row> {
        let idx: Vec<usize> = variables.iter().map(|v| self.var(v)).collect();
        let mut out = Vec::new();
        for row in &input {
            'b: for b in bindings {
                let mut r = row.clone();
                for (k, v) in b.iter().enumerate() {
                    let Some(v) = v else { continue };
                    let t: Term = match v { GroundTerm::NamedNode(n) => n.clone().into(), GroundTerm::Literal(l) => l.clone().into() };
                    let id = self.intern(&t);
                    if r[idx[k]] != UNBOUND && r[idx[k]] != id { continue 'b }
                    r[idx[k]] = id;
                }
                out.push(r);
            }
        }
        out
    }

    fn eval_order_by(&self, inner: &GraphPattern, expression: &[OrderExpression], input: Vec<Row>) -> Vec<Row> {
        let limit = self.order_limit.take();
        let rows = self.eval(inner, input);
        // Sort keys: a plain variable is decoded once per distinct ID; other expressions per row.
        let mut memo: HashMap<TermId, Option<Term>> = HashMap::new();
        let keys: Vec<Vec<Option<Term>>> = rows.iter().map(|r| expression.iter().map(|e| {
            let e = match e { OrderExpression::Asc(e) | OrderExpression::Desc(e) => e };
            match e {
                Expression::Variable(v) => match self.vars.get(v.as_str()).map(|&i| r[i]) {
                    Some(id) if id != UNBOUND => memo.entry(id).or_insert_with(|| Some(self.resolve(id))).clone(),
                    _ => None,
                },
                e => self.value(e, r),
            }
        }).collect()).collect();
        let cmp = |a: &usize, b: &usize| -> std::cmp::Ordering {
            for (k, e) in expression.iter().enumerate() {
                let c = expr::order_cmp(keys[*a][k].as_ref(), keys[*b][k].as_ref());
                let c = if matches!(e, OrderExpression::Desc(_)) { c.reverse() } else { c };
                if c.is_ne() { return c }
            }
            a.cmp(b) // ties keep input order (as the stable sort did)
        };
        let mut idx: Vec<usize> = (0..rows.len()).collect();
        match limit {
            Some(k) if k < idx.len() => {
                // Stable top-k: the comparator is a total order (index tie-break), so the k smallest equal the
                // first k of a full stable sort.
                if k > 0 { idx.select_nth_unstable_by(k - 1, cmp); }
                idx.truncate(k);
                idx.sort_unstable_by(cmp);
            }
            _ => idx.sort_unstable_by(cmp),
        }
        let mut rows: Vec<Option<Row>> = rows.into_iter().map(Some).collect();
        idx.into_iter().map(|i| rows[i].take().unwrap()).collect()
    }

    fn eval_slice(&self, inner: &GraphPattern, start: &usize, length: &Option<usize>, input: Vec<Row>) -> Vec<Row> {
        if let Some(l) = length {
            let direct = match inner {
                GraphPattern::OrderBy { .. } => true,
                GraphPattern::Project { inner, .. } => matches!(inner.as_ref(), GraphPattern::OrderBy { .. }),
                _ => false,
            };
            if direct { self.order_limit.set(Some(start.saturating_add(*l))) }
        }
        let rows = self.eval(inner, input);
        self.order_limit.set(None);
        rows.into_iter().skip(*start).take(length.unwrap_or(usize::MAX)).collect()
    }

    /// `GRAPH <g> P` / `GRAPH ?g P` over the active dataset: P is evaluated with each named graph as the active graph;
    /// `?g` ranges over the dataset's named graphs (never the default graph) and is joined with P's solutions.
    fn eval_graph(&self, name: &NamedNodePattern, inner: &GraphPattern, input: Vec<Row>) -> Vec<Row> {
        let ds = self.dataset.expect("GRAPH is rejected up front without a dataset");
        let view = |g: TermId| ds.graph(g).unwrap_or_else(|e| storage_failure(e));
        match name {
            NamedNodePattern::NamedNode(n) => {
                let Some(id) = self.lookup(&n.clone().into()) else { return Vec::new() };
                if self.named.binary_search(&id).is_err() { return Vec::new() }
                match view(id) { None => Vec::new(), Some(g) => self.with_graph(g, || self.eval(inner, input)) }
            }
            NamedNodePattern::Variable(v) => {
                let i = self.var(v);
                let mut out = Vec::new();
                for &gid in &self.named {
                    let rows: Vec<Row> = input.iter().filter(|r| r[i] == UNBOUND || r[i] == gid).cloned().collect();
                    if rows.is_empty() { continue }
                    let Some(g) = view(gid) else { continue };
                    for mut r in self.with_graph(g, || self.eval(inner, rows)) {
                        if r[i] == UNBOUND { r[i] = gid } else if r[i] != gid { continue }
                        out.push(r);
                    }
                }
                out
            }
        }
    }

    fn eval_group(&self, inner: &GraphPattern, keys: &[Variable], aggs: &[(Variable, AggregateExpression)], input: Vec<Row>) -> Vec<Row> {
        if let Some(rows) = self.try_count_group(inner, keys, aggs, &input) { return rows; }
        let rows = self.eval(inner, input);
        let kidx: Vec<usize> = keys.iter().map(|v| self.var(v)).collect();
        if !self.general_group
            && let Some(out) = self.try_fast_group(&rows, &kidx, aggs) { return out; }
        let mut groups: Vec<(Vec<TermId>, Vec<Row>)> = Vec::new();
        let mut pos: HashMap<Vec<TermId>, usize> = HashMap::new();
        for r in rows {
            let k: Vec<TermId> = kidx.iter().map(|&i| r[i]).collect();
            let g = *pos.entry(k.clone()).or_insert_with(|| { groups.push((k, Vec::new())); groups.len() - 1 });
            groups[g].1.push(r);
        }
        if groups.is_empty() && keys.is_empty() { groups.push((vec![], vec![])); }
        groups.into_iter().map(|(k, members)| {
            let mut out = self.empty_row();
            for (i, &ki) in kidx.iter().enumerate() { out[ki] = k[i]; }
            for (v, agg) in aggs {
                if let Some(t) = self.aggregate(agg, &members) { out[self.var(v)] = self.intern(&t); }
            }
            out
        }).collect()
    }

    fn aggregate(&self, agg: &AggregateExpression, rows: &[Row]) -> Option<Term> {
        match agg {
            AggregateExpression::CountSolutions { distinct } => {
                let n = if *distinct { rows.iter().collect::<HashSet<_>>().len() } else { rows.len() };
                Some(Literal::from(n as i64).into())
            }
            AggregateExpression::FunctionCall { name, expr, distinct } => {
                // SPARQL 1.1 §18.5: the expression is evaluated per solution; an error (including an unbound
                // variable) makes SUM/AVG/MIN/MAX/GROUP_CONCAT an error (unbound result). COUNT counts the non-error
                // values; SAMPLE returns a non-error value. (Matches Jena.)
                // Expressions are deterministic (NOW/RAND/UUID/BNODE are unsupported); EXISTS reads other variables.
                let single = match expr_vars(expr).as_slice() { [v] if !impure(expr) => self.vars.get(v).copied(), _ => None };
                let evaluated: Vec<Option<Term>> = match single {
                    Some(vi) => {
                        let e: &Expression = expr;
                        let addr = e as *const Expression as usize;
                        let eid = {
                            let mut m = self.agg_arg_slots.borrow_mut();
                            match m.get(&addr) {
                                Some((q, slot)) if q == e => *slot,
                                _ => { let slot = self.memo_slots.get(); self.memo_slots.set(slot + 1); m.insert(addr, (e.clone(), slot)); slot }
                            }
                        };
                        rows.iter().map(|r| {
                            let key = (eid, r[vi]);
                            if let Some(v) = self.agg_arg_memo.borrow().get(&key) { return v.clone() }
                            let v = self.value(expr, r);
                            let mut m = self.agg_arg_memo.borrow_mut();
                            if m.len() < MEMO_CAP { m.insert(key, v.clone()); }
                            v
                        }).collect()
                    }
                    None => rows.iter().map(|r| self.value(expr, r)).collect(),
                };
                let has_error = evaluated.iter().any(Option::is_none);
                let mut vals: Vec<Term> = evaluated.into_iter().flatten().collect();
                if *distinct { let mut s = HashSet::new(); vals.retain(|v| s.insert(v.clone())); }
                let strict = !matches!(name, AggregateFunction::Count | AggregateFunction::Sample);
                if strict && has_error { return None }
                match name {
                    AggregateFunction::Count => Some(Literal::from(vals.len() as i64).into()),
                    // Same accumulation as the ID-level fast path (`expr::num_add`): float sums round once.
                    AggregateFunction::Sum => vals.iter().try_fold(expr::Num::Int(0), |a, b| expr::num_add(a, expr::term_num(b)?)).map(|n| n.term()),
                    AggregateFunction::Avg => {
                        if vals.is_empty() { return Some(Literal::from(0i64).into()) }
                        let s = vals.iter().try_fold(expr::Num::Int(0), |a, b| expr::num_add(a, expr::term_num(b)?))?;
                        expr::num_div_count(s, vals.len() as i64).map(|n| n.term())
                    }
                    AggregateFunction::Min => vals.into_iter().min_by(|a, b| expr::order_cmp(Some(a), Some(b))),
                    AggregateFunction::Max => vals.into_iter().max_by(|a, b| expr::order_cmp(Some(a), Some(b))),
                    AggregateFunction::Sample => vals.into_iter().next(),
                    AggregateFunction::GroupConcat { separator } => {
                        let sep = separator.as_deref().unwrap_or(" ");
                        let s: Vec<String> = vals.iter().filter_map(expr::string_value).collect();
                        Some(Literal::new_simple_literal(s.join(sep)).into())
                    }
                    AggregateFunction::Custom(_) => None,
                }
            }
        }
    }

    // ---------- basic graph patterns ----------
    fn slot_of(&self, tp: &TermPattern) -> Slot {
        match tp {
            TermPattern::Variable(v) => Slot::Var(self.var(v)),
            TermPattern::BlankNode(b) => Slot::Var(self.vars[&format!("_:b{}", b.as_str())]),
            TermPattern::NamedNode(n) => Slot::Const(self.lookup(&n.clone().into())),
            TermPattern::Literal(l) => Slot::Const(self.lookup(&l.clone().into())),
        }
    }

    fn eval_bgp(&self, patterns: &[TriplePattern], rows: Vec<Row>) -> Vec<Row> { self.eval_bgp_filtered(patterns, rows, &[]) }

    /// BGP evaluation that applies each filter in `filters` as soon as all its variables are bound.
    fn eval_bgp_filtered(&self, patterns: &[TriplePattern], rows: Vec<Row>, filters: &[(&Expression, Vec<usize>)]) -> Vec<Row> {
        let pats: Vec<[Slot; 3]> = patterns.iter().map(|t| {
            let p = match &t.predicate {
                NamedNodePattern::Variable(v) => Slot::Var(self.var(v)),
                NamedNodePattern::NamedNode(n) => Slot::Const(self.lookup(&n.clone().into())),
            };
            [self.slot_of(&t.subject), p, self.slot_of(&t.object)]
        }).collect();
        // A constant absent from the dictionary can never match.
        if rows.is_empty() || pats.iter().any(|p| p.iter().any(|s| matches!(s, Slot::Const(None)))) { return Vec::new(); }
        let w = self.vars.len();
        if w == 0 {
            // No variables anywhere: every pattern is a membership test.
            let all = pats.iter().all(|p| {
                let ids: Vec<TermId> = p.iter().map(|s| if let Slot::Const(Some(c)) = s { *c } else { UNBOUND }).collect();
                self.count(Perm::Spo, &ids) > 0
            });
            return if all { rows } else { Vec::new() };
        }
        let unbound_start = rows.len() == 1 && rows[0].iter().all(|&x| x == UNBOUND);
        let flat: Vec<TermId> = rows.concat();
        let out = on_data!(self, |g| {
            let bgp = bgp::Bgp { snap: g, w };
            if self.wcoj && unbound_start && pats.len() >= 3 && pats.iter().all(bgp::no_repeated_var) && bgp::cyclic(&pats) {
                let mut out = bgp.leapfrog(&pats, &flat, self.variable_order(&pats));
                if !filters.is_empty() { let all: HashSet<usize> = (0..w).collect(); self.apply_filters(&mut out, w, filters, &all, &mut vec![false; filters.len()]) }
                out
            } else {
                let mut applied = vec![false; filters.len()];
                let mut post = |rows: &mut Vec<TermId>, bound: &HashSet<usize>| self.apply_filters(rows, w, filters, bound, &mut applied);
                if let Some(order) = self.planned_order(&pats, &rows) { bgp.eval_order(&pats, &order, flat, &mut post) }
                else { bgp.eval_greedy(&pats, flat, &mut post) }
            }
        });
        out.chunks_exact(w).map(<[TermId]>::to_vec).collect()
    }

    /// `(?a, ?b)` if `c` is `?a = ?b` / `sameTerm(?a, ?b)` over two distinct variables of the BGP, each of which occurs
    /// only where a literal cannot appear: subject or predicate position, or object of a constant predicate whose
    /// stored objects are all IRIs or blank nodes (checked exactly with one index seek: no object ID at or above the
    /// literal tag). Overlay/delta IDs sit above that tag too, so an unproven store simply keeps the general path.
    fn unifiable_equality(&self, c: &Expression, patterns: &[TriplePattern]) -> Option<(Variable, Variable)> {
        let (x, y) = match c {
            Expression::Equal(x, y) | Expression::SameTerm(x, y) => (x, y),
            _ => return None,
        };
        let (Expression::Variable(a), Expression::Variable(b)) = (x.as_ref(), y.as_ref()) else { return None };
        if a == b { return None }
        let non_literal = |v: &Variable| -> bool {
            let mut seen = false;
            for t in patterns {
                if matches!(&t.subject, TermPattern::Variable(s) if s == v) { seen = true }
                if matches!(&t.predicate, NamedNodePattern::Variable(p) if p == v) { seen = true }
                if matches!(&t.object, TermPattern::Variable(o) if o == v) {
                    seen = true;
                    let NamedNodePattern::NamedNode(p) = &t.predicate else { return false };
                    let Some(pid) = self.lookup(&p.clone().into()) else { return true }; // no triples at all
                    let first_literal = TermId(3 << 60);
                    match on_data!(self, |g| g.seek(Perm::Pos, &[pid], first_literal)) {
                        Ok(None) => {}
                        _ => return false,
                    }
                }
            }
            seen
        };
        (non_literal(a) && non_literal(b)).then(|| (a.clone(), b.clone()))
    }

    /// Applies each not-yet-applied filter whose variables are all bound, keeping rows whose filter is true.
    fn apply_filters(&self, rows: &mut Vec<TermId>, w: usize, filters: &[(&Expression, Vec<usize>)], bound: &HashSet<usize>, applied: &mut [bool]) {
        for (k, (e, vs)) in filters.iter().enumerate() {
            if applied[k] || !vs.iter().all(|v| bound.contains(v)) { continue }
            applied[k] = true;
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
            let mut memo: HashMap<TermId, Option<expr::Num>> = HashMap::new();
            let mut val = |id: TermId| *memo.entry(id).or_insert_with(|| expr::term_num(&self.resolve(id)));
            // A filter over one variable (and no EXISTS) depends only on that column's term: memoize its result per
            // term ID, so e.g. a date range over many reviews decodes each distinct date once.
            let single = match vs.as_slice() { [c] if !impure(e) => Some(*c), _ => None };
            let mut by_id: HashMap<TermId, bool> = HashMap::new();
            for r in rows.chunks_exact(w) {
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
        }
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

    /// Leapfrog variable order from the optimizer's cost model (task #5), when statistics are available.
    fn variable_order(&self, pats: &[[Slot; 3]]) -> Option<Vec<usize>> {
        if self.data.get().is_some() { return None }
        let (stats, _) = self.stats?;
        if pats.len() > opt::planner::MAX_RUNTIME_DP_PATTERNS { return None; }
        let conv = |s: &Slot| match *s { Slot::Const(c) => opt::plan::Slot::Const(c.unwrap()), Slot::Var(v) => opt::plan::Slot::Var(v as u32) };
        let tps: Vec<opt::plan::TriplePattern> = pats.iter().map(|p| opt::plan::TriplePattern { s: conv(&p[0]), p: conv(&p[1]), o: conv(&p[2]) }).collect();
        if opt::planner::estimated_greedy_work(&tps, stats) < opt::planner::MIN_OPTIMIZER_WORK { return None; }
        Some(opt::planner::choose_variable_order(&tps, stats).into_iter().map(|v| v as usize).collect())
    }

    /// Pattern order from the cost-based planner: leaves of its join tree, left to right. Used for top-level BGPs
    /// (a single all-unbound input row); nested BGPs with bound inputs keep the greedy order.
    fn planned_order(&self, pats: &[[Slot; 3]], rows: &[Row]) -> Option<Vec<usize>> {
        if self.data.get().is_some() { return None }
        let (stats, dynsnap) = self.stats?;
        if pats.len() < 2 || rows.len() != 1 || rows[0].iter().any(|&x| x != UNBOUND) { return None }
        // Explicit diagnostic only; normal requests do not acquire clocks or emit output.
        let trace = std::env::var_os("TRIPLELIGHT_PROFILE_PLAN").is_some();
        let clock = || trace.then(std::time::Instant::now);
        let elapsed = |start: Option<std::time::Instant>| start.map_or(0, |s| s.elapsed().as_nanos());
        let total_clock = clock();
        let conv = |s: &Slot| match *s { Slot::Const(c) => opt::plan::Slot::Const(c.unwrap()), Slot::Var(v) => opt::plan::Slot::Var(v as u32) };
        let tps: Vec<opt::plan::TriplePattern> = pats.iter().map(|p| opt::plan::TriplePattern { s: conv(&p[0]), p: conv(&p[1]), o: conv(&p[2]) }).collect();
        if pats.len() > opt::planner::MAX_RUNTIME_DP_PATTERNS {
            if trace {
                eprintln!("planning_search kind=runtime-greedy patterns={}", pats.len());
                eprintln!("planning_profile clone_ns=0 prefix_ns=0 search_ns=0 gate_ns=0 total_ns={}", elapsed(total_clock));
            }
            return None;
        }
        let gate_clock = clock();
        let work = opt::planner::estimated_greedy_work(&tps, stats);
        let gate_ns = elapsed(gate_clock);
        if trace { eprintln!("planner_gate work={work:.3} enabled={}", work >= opt::planner::MIN_OPTIMIZER_WORK); }
        if work < opt::planner::MIN_OPTIMIZER_WORK {
            if trace { eprintln!("planning_profile clone_ns=0 prefix_ns=0 search_ns=0 gate_ns={gate_ns} total_ns={}", elapsed(total_clock)); }
            return None;
        }
        let phase = clock();
        let mut st = stats.clone();
        let clone_ns = elapsed(phase);
        let phase = clock();
        st.cache_pattern_counts(&tps, dynsnap).unwrap_or_else(|e| storage_failure(std::io::Error::other(e)));
        let prefix_ns = elapsed(phase);
        let phase = clock();
        let tree = opt::planner::plan_bgp(&tps, &st);
        let search_ns = elapsed(phase);
        fn leaves(t: &opt::plan::JoinTree, out: &mut Vec<opt::plan::TriplePattern>) {
            use opt::plan::JoinTree::*;
            match t {
                Scan { pattern, .. } => out.push(*pattern),
                IndexNestedLoop { outer, pattern, .. } => { leaves(outer, out); out.push(*pattern); }
                MergeJoin { left, right, .. } => { leaves(left, out); leaves(right, out) }
                HashJoin { build, probe, .. } => { leaves(build, out); leaves(probe, out) }
                Leapfrog { children, .. } => for c in children { leaves(c, out) },
                Unit | Empty => {}
            }
        }
        let mut ls = Vec::new();
        leaves(&tree, &mut ls);
        let mut used = vec![false; tps.len()];
        let order: Vec<usize> = ls.iter().filter_map(|l| {
            let i = (0..tps.len()).find(|&i| !used[i] && tps[i] == *l)?; used[i] = true; Some(i)
        }).collect();
        if trace { eprintln!("planning_profile clone_ns={clone_ns} prefix_ns={prefix_ns} search_ns={search_ns} gate_ns={gate_ns} total_ns={}", elapsed(total_clock)); }
        (order.len() == tps.len()).then_some(order)
    }

    pub(crate) fn ebv(&self, e: &Expression, r: &Row) -> bool {
        self.value(e, r).and_then(|t| expr::ebv(&t)).unwrap_or(false)
    }
    pub(crate) fn value(&self, e: &Expression, r: &Row) -> Option<Term> { expr::eval(self, e, r) }
    pub(crate) fn var_value(&self, v: &Variable, r: &Row) -> Option<Term> {
        let id = *r.get(*self.vars.get(v.as_str())?)?;
        (id != UNBOUND).then(|| self.resolve(id))
    }
    /// `FILTER [NOT] EXISTS { P }` over many rows as one batched evaluation (a semi-/anti-join): P is evaluated
    /// once with one input row per distinct binding of P's variables, and each row is kept by set membership.
    /// Only for P built from BGPs, joins and EXISTS-free filters, whose evaluation distributes over input rows.
    fn exists_batch(&self, expr: &Expression, rows: &[Row]) -> Option<Vec<Row>> {
        let (neg, p) = match expr {
            Expression::Exists(p) => (false, &**p),
            Expression::Not(e) => match &**e { Expression::Exists(p) => (true, &**p), _ => return None },
            _ => return None,
        };
        if rows.len() < 64 || !distributive(p) { return None }
        let mut names = Vec::new();
        walk_vars(p, &mut names);
        let w = rows[0].len();
        let idx: Vec<usize> = names.iter().filter_map(|n| self.vars.get(n).copied()).filter(|&i| i < w).collect();
        let key: Vec<usize> = idx.iter().copied().filter(|&i| rows[0][i] != UNBOUND).collect();
        // Every row must bind exactly the same P variables (so one key layout describes all of them).
        if rows.iter().any(|r| idx.iter().any(|&i| (r[i] != UNBOUND) != key.contains(&i))) { return None }
        let mut reps: Vec<Row> = Vec::new();
        let mut seen: HashSet<Vec<TermId>> = HashSet::new();
        for r in rows {
            let k: Vec<TermId> = key.iter().map(|&i| r[i]).collect();
            if seen.insert(k) { let mut x = self.empty_row(); for &i in &key { x[i] = r[i] } reps.push(x) }
        }
        let hits: HashSet<Vec<TermId>> = self.eval(p, reps).iter().map(|r| key.iter().map(|&i| r[i]).collect()).collect();
        Some(rows.iter().filter(|r| hits.contains(&key.iter().map(|&i| r[i]).collect::<Vec<_>>()) != neg).cloned().collect())
    }

    pub(crate) fn exists(&self, p: &GraphPattern, r: &Row) -> bool {
        let addr = p as *const GraphPattern as usize;
        let (slot, idx) = {
            let mut m = self.exists_vars.borrow_mut();
            match m.get(&addr) {
                Some((q, slot, idx)) if q == p => (*slot, idx.clone()),
                _ => {
                    // GRAPH/SERVICE names are not visited by `walk_vars`; never memoize those patterns.
                    let idx = memo_safe(p).then(|| {
                        let mut v = Vec::new();
                        walk_vars(p, &mut v);
                        v.iter().filter_map(|n| self.vars.get(n).copied()).filter(|&i| i < r.len()).collect::<Vec<_>>()
                    });
                    let slot = self.memo_slots.get(); self.memo_slots.set(slot + 1);
                    m.insert(addr, (p.clone(), slot, idx.clone()));
                    (slot, idx)
                }
            }
        };
        let Some(idx) = idx else { return !self.eval(p, vec![r.clone()]).is_empty() };
        let key = (slot, self.graph_token(), idx.iter().map(|&i| r[i]).collect::<Vec<_>>());
        if let Some(&b) = self.exists_memo.borrow().get(&key) { return b }
        let b = !self.eval(p, vec![r.clone()]).is_empty();
        let mut m = self.exists_memo.borrow_mut();
        if m.len() < MEMO_CAP { m.insert(key, b); }
        b
    }
}

/// Upper bound on entries in each per-query memo (EXISTS results, aggregate arguments).
const MEMO_CAP: usize = 1 << 20;

/// Marker payload for storage failures raised inside evaluation.
struct StorageFailure(String);
fn storage_failure(e: crate::storage::StorageError) -> ! { std::panic::panic_any(StorageFailure(e.to_string())) }

/// Compatible-mapping join (used to merge subquery results back into the outer rows).
/// Compatible-mapping join of two row sets. Columns bound in every row on both sides are a sound hash key: two
/// compatible mappings must agree there. The remaining columns are merged with the usual compatibility check. Without
/// such a column (partial or unbound mappings), the nested-loop join is used.
fn join_rows(left: Vec<Row>, right: Vec<Row>) -> Vec<Row> {
    if left.is_empty() || right.is_empty() { return Vec::new() }
    let width = left[0].len();
    let key: Vec<usize> = (0..width).filter(|&i| left.iter().all(|r| r[i] != UNBOUND) && right.iter().all(|r| r[i] != UNBOUND)).collect();
    if !key.is_empty() && left.len().saturating_mul(right.len()) > 64 {
        let mut table: HashMap<Vec<TermId>, Vec<usize>> = HashMap::new();
        for (j, b) in right.iter().enumerate() { table.entry(key.iter().map(|&i| b[i]).collect()).or_default().push(j) }
        let mut out = Vec::new();
        let mut k: Vec<TermId> = Vec::with_capacity(key.len());
        for a in &left {
            k.clear(); k.extend(key.iter().map(|&i| a[i]));
            let Some(matches) = table.get(k.as_slice()) else { continue };
            'm: for &j in matches {
                let b = &right[j];
                let mut r = a.clone();
                for (i, &y) in b.iter().enumerate() {
                    if y == UNBOUND { continue }
                    if r[i] == UNBOUND { r[i] = y } else if r[i] != y { continue 'm }
                }
                out.push(r);
            }
        }
        return out;
    }
    let mut out = Vec::new();
    for a in &left {
        'r: for b in &right {
            let mut r = a.clone();
            for (i, &y) in b.iter().enumerate() {
                if y == UNBOUND { continue }
                if r[i] == UNBOUND { r[i] = y } else if r[i] != y { continue 'r }
            }
            out.push(r);
        }
    }
    out
}

fn distributive(p: &GraphPattern) -> bool {
    match p {
        GraphPattern::Bgp { .. } => true,
        GraphPattern::Join { left, right } => distributive(left) && distributive(right),
        GraphPattern::Filter { inner, expr } => distributive(inner) && !impure(expr),
        _ => false,
    }
}

/// Whether evaluating `p` with outer solutions passed in (sideways information passing) equals evaluating it on its
/// own and joining: true when every FILTER / BIND / OPTIONAL condition inside `p` only reads variables bound by the
/// pattern it applies to. SPARQL evaluates groups bottom-up, so e.g. `{ BIND(?o+1 AS ?z) }` must not see an outer ?o.
fn scope_safe(p: &GraphPattern) -> bool {
    let bound = certainly_bound;
    let reads_only = |e: &Expression, q: &GraphPattern| { let b = bound(q); expr_vars(e).iter().all(|n| b.contains(n)) };
    match p {
        GraphPattern::Bgp { .. } | GraphPattern::Path { .. } | GraphPattern::Values { .. } => true,
        GraphPattern::Filter { expr, inner } => scope_safe(inner) && reads_only(expr, inner),
        GraphPattern::Extend { inner, expression, .. } => scope_safe(inner) && reads_only(expression, inner),
        GraphPattern::Join { left, right } | GraphPattern::Union { left, right } | GraphPattern::Minus { left, right } => scope_safe(left) && scope_safe(right),
        GraphPattern::LeftJoin { left, right, expression } => scope_safe(left) && scope_safe(right) && expression.as_ref().is_none_or(|e| {
            let (bl, br) = (bound(left), bound(right));
            expr_vars(e).iter().all(|n| bl.contains(n) || br.contains(n))
        }),
        // LIMIT/OFFSET picks rows from the subquery's own solutions; restricting its input first changes which rows
        // are picked (W3C subquery11).
        GraphPattern::Slice { .. } => false,
        GraphPattern::OrderBy { inner, .. } | GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner }
        | GraphPattern::Project { inner, .. } | GraphPattern::Group { inner, .. } | GraphPattern::Graph { inner, .. } | GraphPattern::Service { inner, .. } => scope_safe(inner),
    }
}

/// Variables bound in every solution of `p` (an under-approximation): a variable that may be unbound (VALUES UNDEF,
/// OPTIONAL, one UNION branch, a BIND whose expression can fail) is excluded, so an outer value can never leak into it.
fn certainly_bound(p: &GraphPattern) -> HashSet<String> {
    match p {
        GraphPattern::Bgp { patterns } => {
            let mut v = Vec::new();
            walk_vars(&GraphPattern::Bgp { patterns: patterns.clone() }, &mut v);
            v.into_iter().collect()
        }
        GraphPattern::Path { subject, object, .. } => [subject, object].into_iter().filter_map(|t| match t {
            TermPattern::Variable(v) => Some(v.as_str().to_string()),
            _ => None,
        }).collect(),
        GraphPattern::Values { variables, bindings } => variables.iter().enumerate()
            .filter(|(i, _)| bindings.iter().all(|row| row.get(*i).is_some_and(Option::is_some)))
            .map(|(_, v)| v.as_str().to_string()).collect(),
        GraphPattern::Join { left, right } => { let mut a = certainly_bound(left); a.extend(certainly_bound(right)); a }
        GraphPattern::Union { left, right } => { let b = certainly_bound(right); certainly_bound(left).into_iter().filter(|v| b.contains(v)).collect() }
        GraphPattern::LeftJoin { left, .. } | GraphPattern::Minus { left, .. } => certainly_bound(left),
        GraphPattern::Filter { inner, .. } | GraphPattern::Extend { inner, .. } | GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner } | GraphPattern::Slice { inner, .. }
        | GraphPattern::Service { inner, .. } => certainly_bound(inner),
        GraphPattern::Graph { name, inner } => {
            let mut b = certainly_bound(inner);
            if let NamedNodePattern::Variable(v) = name { b.insert(v.as_str().to_string()); }
            b
        }
        GraphPattern::Project { inner, variables } => { let b = certainly_bound(inner); variables.iter().map(|v| v.as_str().to_string()).filter(|v| b.contains(v)).collect() }
        // Group keys certainly bound inside, plus the aggregate output variables: those are fresh names set by the
        // aggregation itself, so no outer solution can supply a value for them.
        GraphPattern::Group { inner, variables, aggregates } => {
            let b = certainly_bound(inner);
            let mut out: HashSet<String> = variables.iter().map(|v| v.as_str().to_string()).filter(|v| b.contains(v)).collect();
            out.extend(aggregates.iter().map(|(v, _)| v.as_str().to_string()));
            out
        }
    }
}

fn memo_safe(p: &GraphPattern) -> bool {
    match p {
        GraphPattern::Bgp { .. } | GraphPattern::Path { .. } | GraphPattern::Values { .. } => true,
        GraphPattern::Graph { .. } | GraphPattern::Service { .. } => false,
        GraphPattern::Join { left, right } | GraphPattern::Union { left, right } | GraphPattern::Minus { left, right }
        => memo_safe(left) && memo_safe(right),
        GraphPattern::LeftJoin { left, right, expression } => memo_safe(left) && memo_safe(right) && !expression.as_ref().is_some_and(impure),
        GraphPattern::Filter { inner, expr } => memo_safe(inner) && !impure(expr),
        GraphPattern::Extend { inner, expression, .. } => memo_safe(inner) && !impure(expression),
        GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner } | GraphPattern::Slice { inner, .. }
        | GraphPattern::Project { inner, .. } | GraphPattern::Group { inner, .. } => memo_safe(inner),
    }
}

fn walk_vars(p: &GraphPattern, out: &mut Vec<String>) {
    let mut add = |s: String| if !out.contains(&s) { out.push(s) };
    match p {
        GraphPattern::Bgp { patterns } => for t in patterns {
            for tp in [&t.subject, &t.object] {
                match tp {
                    TermPattern::Variable(v) => add(v.as_str().to_string()),
                    TermPattern::BlankNode(b) => add(format!("_:b{}", b.as_str())),
                    _ => {}
                }
            }
            if let NamedNodePattern::Variable(v) = &t.predicate { add(v.as_str().to_string()) }
        },
        GraphPattern::Path { subject, object, .. } => for tp in [subject, object] {
            match tp {
                TermPattern::Variable(v) => add(v.as_str().to_string()),
                // The parser splits `a (p|q)/r ?x` into two paths joined on a fresh blank node.
                TermPattern::BlankNode(b) => add(format!("_:b{}", b.as_str())),
                _ => {}
            }
        },
        GraphPattern::Join { left, right } | GraphPattern::Union { left, right } | GraphPattern::Minus { left, right }
        => { walk_vars(left, out); walk_vars(right, out) }
        GraphPattern::LeftJoin { left, right, expression } => {
            walk_vars(left, out); walk_vars(right, out);
            if let Some(e) = expression { walk_expr(e, out) }
        }
        GraphPattern::Filter { expr, inner } => { walk_vars(inner, out); walk_expr(expr, out) }
        GraphPattern::Extend { inner, variable, expression } => {
            walk_vars(inner, out); walk_expr(expression, out);
            let s = variable.as_str().to_string(); if !out.contains(&s) { out.push(s) }
        }
        GraphPattern::Values { variables, .. } => for v in variables { add(v.as_str().to_string()) },
        GraphPattern::OrderBy { inner, .. } | GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. } | GraphPattern::Service { inner, .. } => walk_vars(inner, out),
        GraphPattern::Graph { name, inner } => {
            if let NamedNodePattern::Variable(v) = name { add(v.as_str().to_string()) }
            walk_vars(inner, out)
        }
        GraphPattern::Project { inner, variables } => {
            walk_vars(inner, out);
            for v in variables { let s = v.as_str().to_string(); if !out.contains(&s) { out.push(s) } }
        }
        GraphPattern::Group { inner, variables, aggregates } => {
            walk_vars(inner, out);
            for v in variables { let s = v.as_str().to_string(); if !out.contains(&s) { out.push(s) } }
            for (v, _) in aggregates { let s = v.as_str().to_string(); if !out.contains(&s) { out.push(s) } }
        }
    }
}

fn walk_expr(e: &Expression, out: &mut Vec<String>) {
    match e {
        Expression::Exists(p) => walk_vars(p, out),
        Expression::Variable(v) | Expression::Bound(v) => { let s = v.as_str().to_string(); if !out.contains(&s) { out.push(s) } }
        e => for c in e.children() { walk_expr(c, out) },
    }
}

/// The change set of a SPARQL 1.1 Update request, computed against `snap` (task #8). Supports INSERT DATA,
/// DELETE DATA and DELETE/INSERT ... WHERE on the default graph. Every operation's WHERE clause is evaluated on
/// `snap` (the state before the request); deletions of an operation precede its insertions.
pub fn update_changes<S: Snapshot>(snap: &S, text: &str) -> Result<Vec<(crate::store::Change, oxrdf::Triple)>, QueryError> {
    use crate::store::Change;
    use oxrdf::{NamedOrBlankNode, Triple};
    use crate::sparql::GraphName;
    use crate::sparql::{GraphNamePattern, GroundTermPattern};
    use crate::sparql::GraphUpdateOperation as Op;
    let unsupported = |what: &str| QueryError::new(QueryErrorKind::Unsupported, format!("{what} is not supported yet"));
    let u = crate::sparql::parse_update(text, None).map_err(|e| QueryError::new(QueryErrorKind::Syntax, e.to_string()))?;
    let mut out = Vec::new();
    for op in &u.operations {
        match op {
            Op::InsertData { data } => for q in data {
                if q.graph_name != GraphName::DefaultGraph { return Err(unsupported("named graphs")) }
                out.push((Change::Insert, Triple::new(q.subject.clone(), q.predicate.clone(), q.object.clone())));
            },
            Op::DeleteData { data } => for q in data {
                if q.graph_name != GraphName::DefaultGraph { return Err(unsupported("named graphs")) }
                let o: Term = match &q.object { GroundTerm::NamedNode(n) => n.clone().into(), GroundTerm::Literal(l) => l.clone().into() };
                out.push((Change::Delete, Triple::new(q.subject.clone(), q.predicate.clone(), o)));
            },
            Op::DeleteInsert { delete, insert, with, using, pattern } => {
                if using.is_some() { return Err(unsupported("USING")) }
                if with.is_some() { return Err(unsupported("WITH (named graphs)")) }
                if delete.iter().any(|d| !matches!(d.graph_name, GraphNamePattern::DefaultGraph))
                    || insert.iter().any(|d| !matches!(d.graph_name, GraphNamePattern::DefaultGraph)) { return Err(unsupported("named graphs")) }
                let q = Query::Select { dataset: None, pattern: (**pattern).clone(), base_iri: None };
                let QueryResult::Solutions { vars, rows } = run_query(snap, None, &q, &Options::default())? else { unreachable!() };
                let col: HashMap<&str, usize> = vars.iter().enumerate().map(|(i, v)| (v.as_str(), i)).collect();
                for (n, row) in rows.iter().enumerate() {
                    let var = |v: &Variable| col.get(v.as_str()).and_then(|&i| row[i].clone());
                    let ground = |t: &GroundTermPattern| -> Option<Term> { match t {
                        GroundTermPattern::NamedNode(x) => Some(x.clone().into()),
                        GroundTermPattern::Literal(x) => Some(x.clone().into()),
                        GroundTermPattern::Variable(v) => var(v),
                    } };
                    let term = |t: &TermPattern| -> Option<Term> { match t {
                        TermPattern::NamedNode(x) => Some(x.clone().into()),
                        TermPattern::Literal(x) => Some(x.clone().into()),
                        TermPattern::BlankNode(b) => Some(BlankNode::new_unchecked(format!("{}u{n}", b.as_str())).into()),
                        TermPattern::Variable(v) => var(v),
                    } };
                    let pred = |p: &NamedNodePattern| -> Option<oxrdf::NamedNode> { match p {
                        NamedNodePattern::NamedNode(x) => Some(x.clone()),
                        NamedNodePattern::Variable(v) => match var(v) { Some(Term::NamedNode(x)) => Some(x), _ => None },
                    } };
                    // Templates with an unbound or ill-typed slot produce no triple for this solution.
                    for d in delete {
                        if let (Some(s), Some(p), Some(o)) = (ground(&d.subject), pred(&d.predicate), ground(&d.object))
                            && let Ok(s) = NamedOrBlankNode::try_from(s) { out.push((Change::Delete, Triple::new(s, p, o))) }
                    }
                    for i in insert {
                        if let (Some(s), Some(p), Some(o)) = (term(&i.subject), pred(&i.predicate), term(&i.object))
                            && let Ok(s) = NamedOrBlankNode::try_from(s) { out.push((Change::Insert, Triple::new(s, p, o))) }
                    }
                }
            }
            Op::Management { operation } => return Err(unsupported(operation)),
        }
    }
    Ok(out)
}

fn conjuncts(e: &Expression) -> Vec<&Expression> {
    match e { Expression::And(a, b) => { let mut v = conjuncts(a); v.extend(conjuncts(b)); v } e => vec![e] }
}
fn expr_vars(e: &Expression) -> Vec<String> { let mut v = Vec::new(); walk_expr(e, &mut v); v }
/// True if `e` contains EXISTS or a function whose value differs between calls (RAND, UUID, STRUUID, BNODE). Such an
/// expression depends on more than the IDs of its variables, so it must not be memoized per ID, unified or moved.
fn impure(e: &Expression) -> bool {
    match e {
        Expression::Exists(_) => true,
        Expression::FunctionCall(Function::Rand | Function::Uuid | Function::StrUuid | Function::BNode, _) => true,
        e => e.children().any(impure),
    }
}

/// `t` with variable `from` replaced by `to` (subject, predicate and object positions).
fn rename_var(t: &TriplePattern, from: &Variable, to: &Variable) -> TriplePattern {
    let tp = |x: &TermPattern| match x { TermPattern::Variable(v) if v == from => TermPattern::Variable(to.clone()), x => x.clone() };
    let pp = match &t.predicate { NamedNodePattern::Variable(v) if v == from => NamedNodePattern::Variable(to.clone()), p => p.clone() };
    TriplePattern { subject: tp(&t.subject), predicate: pp, object: tp(&t.object) }
}
