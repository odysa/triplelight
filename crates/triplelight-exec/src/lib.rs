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

pub mod phase;
#[doc(hidden)]
pub use expr::udf;
#[doc(hidden)]
pub use expr::builtin_iri;
pub mod plan_trace;
mod agg;
mod bgp;
mod id_hash;
mod join;
mod limits;
use join::join_rows;
mod views;
use views::{dataset, Views};
mod path;
mod pattern;
mod range;
mod expr;
mod support;
#[cfg(feature = "rdf-12")]
mod triple_term;
use pattern::Slot;
pub mod results;

use crate::model::{Perm, TermId};
use crate::storage::{DatasetView, Snapshot};
use crate::opt::stats::Stats;
use triplelight_rdf::{BlankNode, Literal, Term, Variable};
use crate::sparql::{AggregateExpression, AggregateFunction, Expression, Function, GraphPattern, OrderExpression};
use crate::sparql::{GroundTerm, NamedNodePattern, TermPattern, TriplePattern};
use crate::sparql::Query;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

pub const UNBOUND: TermId = TermId(0);
const SCRATCH: u64 = crate::model::id_tag::SCRATCH << 60;

pub type Row = Vec<TermId>;

#[derive(Debug)]
pub enum QueryResult {
    Solutions { vars: Vec<Variable>, rows: Vec<Vec<Option<Term>>> },
    /// Undecoded solutions (with `Options::raw_ids`): serialize with `results::serialize_ids`.
    Ids(IdSolutions),
    Boolean(bool),
    Graph(Vec<triplelight_rdf::Triple>),
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
pub enum QueryErrorKind {
    Syntax,
    Unsupported,
    Storage,
    /// An update names a graph in the wrong state: `CREATE` of an existing graph, or `DROP`, `CLEAR`, `ADD`, `MOVE`
    /// or `COPY` of an absent one, without `SILENT`. The request is at fault, not the store.
    GraphState,
    /// The caller cancelled the query (`Interrupt::cancel`) while it ran. No partial answer is returned.
    Cancelled,
    /// The query ran past its deadline (`Interrupt::with_timeout`). No partial answer is returned.
    Timeout,
    /// The query needed more working memory than its `MemoryBudget`. No partial answer is returned.
    ResourceLimit,
}

pub use limits::{Interrupt, MemoryBudget, OverBudget, QueryLimits, Reservation};
pub use limits::spill::sweep_stale as sweep_stale_spills;
use limits::{Interrupted, OverBudgetStop};

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
    /// Answer eligible aggregates by exact index counts (`try_count_group`: index-range counts, last-level counts,
    /// products of disconnected parts). Turning it off evaluates them by full enumeration with identical answers;
    /// it exists for ablations and differential checks.
    pub count_pushdown: bool,
    /// Range-filter pushdown (`range.rs`): a pattern `?s <p> ?d` under FILTER bounds on `?d` with `xsd:date`
    /// constants scans only the object IDs that can pass them, when the store orders literal IDs by text and `<p>`'s
    /// dates are canonical, the range holds at most 15% of `<p>`'s triples, and the BGP's solution order cannot
    /// reach the answers (no SUM/AVG/MIN/MAX/SAMPLE/GROUP_CONCAT above it, no LIMIT/OFFSET over an ORDER BY that may
    /// tie). The filter still runs, so turning it off gives the same answers; it exists for ablations
    /// (`--ablate=range`) and differential checks.
    pub range_pushdown: bool,
    /// Let the cost-based planner use the statistics' characteristic pairs (`Stats::set_char_pairs`) to estimate
    /// subject-to-object joins between stars. Plans only; off (`--ablate=charpairs`) gives the pre-pairs plans.
    pub char_pairs: bool,
    /// Sort-then-probe (`bgp::Bgp::extend_sorted`): a probe join of at least 16k rows with a single bound key probes
    /// them in key order and restores the input order afterwards. Answers are identical; off
    /// (`--ablate=sortprobe`) probes in input order.
    pub sorted_probe: bool,
    /// CSR adjacency for `p+` / `p*` / `^p+` / `^p*` over one predicate (`path.rs`): the predicate's edges are read
    /// once per query and active graph into a compressed sparse row array and the closures run over it (all-pairs
    /// closures in parallel) instead of one index seek per visited node. Above a size cap it keeps the seeks.
    /// Answers and their order are identical; off (`--ablate=csrpaths`) always seeks.
    pub csr_paths: bool,
    /// Which entailment regime basic graph patterns are matched under. `Simple` (the default) matches
    /// the stored triples; `Rdfs` matches the default graph's RDFS closure, computed at query time.
    pub entailment: Entailment,
    /// Cancellation and deadline of this query (`Interrupt`); `None` runs to completion.
    pub interrupt: Option<&'a Interrupt>,
    /// The working-memory budget of this query (`MemoryBudget`); `None` is unbounded.
    pub memory_budget: Option<&'a MemoryBudget>,
}
impl Default for Options<'_> { fn default() -> Self { Options { stats: None, wcoj: true, raw_ids: false, general_group: false, count_pushdown: true, range_pushdown: true, char_pairs: true, sorted_probe: true, csr_paths: true, entailment: Entailment::Simple, interrupt: None, memory_budget: None } } }

/// The entailment regime of a query's graph matching.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Entailment {
    /// Simple entailment: the stored triples, as SPARQL specifies by default.
    #[default]
    Simple,
    /// RDFS: subclass and subproperty transitivity, type and property propagation, domain and range (the rule set of
    /// Jena's RDFS reasoner at level `simple`, `triplelight-rdfs`). Each graph of the dataset is entailed on its own triples:
    /// the default graph (a FROM merge: the merge), and each named graph under GRAPH. Requires the `rdfs` feature.
    Rdfs,
}

pub fn query_with<S: Snapshot>(snap: &S, text: &str, opts: &Options<'_>) -> Result<QueryResult, QueryError> {
    query_with_extensions(snap, None, text, opts, udf::QueryExtensions::default())
}

/// Storage reads inside evaluation unwind with `StorageFailure`; turn that into a `Storage` error.
fn catching_storage<T>(f: impl FnOnce() -> Result<T, QueryError>) -> Result<T, QueryError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => match p.downcast::<StorageFailure>() {
            Ok(f) => Err(QueryError::new(QueryErrorKind::Storage, format!("storage error: {}", f.0))),
            Err(p) => match p.downcast::<Interrupted>() {
                Ok(i) => Err(QueryError::new(i.0, if i.0 == QueryErrorKind::Timeout { "query timed out" } else { "query cancelled" })),
                Err(p) => match p.downcast::<OverBudgetStop>() {
                    Ok(o) => Err(QueryError::new(QueryErrorKind::ResourceLimit, format!(
                        "query needs more memory than its budget: {} more bytes requested, {} available", o.0.requested, o.0.available))),
                    Err(p) => std::panic::resume_unwind(p),
                },
            },
        },
    }
}

fn run<S: Snapshot>(snap: &S, dataset: Option<&dyn DatasetView>, text: &str, opts: &Options<'_>, extensions: udf::QueryExtensions<'_>) -> Result<QueryResult, QueryError> {
    let q = phase::parse(|| crate::sparql::parse_query(text, None)).map_err(|e| QueryError::new(QueryErrorKind::Syntax, e.to_string()))?;
    run_query(snap, dataset, &q, opts, extensions)
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
    query_with_extensions(snap, Some(dataset), text, opts, udf::QueryExtensions::default())
}


/// Evaluates a dataset query using an entailment provider selected by the caller.
pub fn query_dataset_with_provider<S: Snapshot>(
    snap: &S,
    dataset: &dyn DatasetView,
    text: &str,
    opts: &Options<'_>,
    provider: &dyn storage::EntailmentProvider,
) -> Result<QueryResult, QueryError> {
    query_with_extensions(snap, Some(dataset), text, opts, udf::QueryExtensions { entailment: Some(provider), ..Default::default() })
}

#[doc(hidden)]
pub fn query_with_extensions<S: Snapshot>(
    snap: &S,
    dataset: Option<&dyn DatasetView>,
    text: &str,
    opts: &Options<'_>,
    extensions: udf::QueryExtensions<'_>,
) -> Result<QueryResult, QueryError> {
    catching_storage(|| run(snap, dataset, text, opts, extensions))
}

fn run_query<S: Snapshot>(snap: &S, dataset: Option<&dyn DatasetView>, q: &Query, opts: &Options<'_>, extensions: udf::QueryExtensions<'_>) -> Result<QueryResult, QueryError> {
    // Reject what we do not evaluate instead of answering it wrongly (ignored GRAPH, a function that errors per row).
    if let Some(reason) = support::unsupported(q, dataset.is_some(), extensions.functions) { return Err(QueryError::new(QueryErrorKind::Unsupported, reason)) }
    // Direct executor callers may request the linked RDFS provider without an embedded registry.
    let provider = if extensions.entailment.is_none() && opts.entailment == Entailment::Rdfs {
        #[cfg(feature = "rdfs")]
        { Some(&triplelight_rdfs::Rdfs as &dyn storage::EntailmentProvider) }
        #[cfg(not(feature = "rdfs"))]
        { return Err(QueryError::new(QueryErrorKind::Unsupported, "RDFS entailment requires the rdfs Cargo feature")); }
    } else {
        extensions.entailment
    };
    let spec = match q { Query::Select { dataset, .. } | Query::Ask { dataset, .. } | Query::Construct { dataset, .. } | Query::Describe { dataset, .. } => dataset };
    // Graph views for the active dataset; they outlive `ctx`, which borrows them.
    let (from, named) = match dataset { Some(ds) => dataset::resolve(snap, ds, spec.as_ref())?, None => (Vec::new(), Vec::new()) };
    let empty = dataset::EmptyGraph { dict: snap as &dyn Snapshot };
    let union = (from.len() > 1).then(|| dataset::UnionGraph { members: from.clone() });
    let default: Option<&dyn Snapshot> = if spec.is_some() && dataset.is_some() {
        Some(match (from.len(), &union) {
            (0, _) => &empty,
            (1, _) => from[0],
            (_, Some(u)) => u,
            _ => unreachable!(),
        })
    } else {
        None
    };
    // The provider sees each active graph separately, including the merged default graph for FROM.
    let views = Views::new(provider, snap as &dyn Snapshot, default, dataset, &named).unwrap_or_else(|e| storage_failure(e));
    let mut ctx = Ctx::for_query(snap, dataset, &views, named, q, opts, extensions, path::CsrLimits::default());
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
    // Template blank nodes get a fresh node per solution, labelled `<template label>r<row>`. Those labels must not
    // collide with a blank node from the data that a solution binds (e.g. an input `_:tr0`), or one output node could
    // stand for two different nodes. Collect the labels of every blank node the solutions bind, stored or computed.
    // A template without blank nodes generates no labels, so there is nothing to reserve.
    #[cfg(not(feature = "rdf-12"))]
    let has_template_blanks = template.iter().any(|t| matches!(t.subject, TermPattern::BlankNode(_)) || matches!(t.object, TermPattern::BlankNode(_)));
    // Blank nodes nested in template triple terms are fresh per solution too.
    #[cfg(feature = "rdf-12")]
    let has_template_blanks = template.iter().any(|t| triple_term::has_blank(&t.subject) || triple_term::has_blank(&t.object));
    let mut bound_ids = HashSet::new();
    if has_template_blanks {
        #[cfg(not(feature = "rdf-12"))]
        for r in &rows { for &id in r { if id != UNBOUND && matches!(id.0 >> 60, 2 | 0xe) { bound_ids.insert(id); } } }
        // Triple terms (tag 4) can carry blank nodes too; overlay delta IDs (tag 0xd) can be either.
        #[cfg(feature = "rdf-12")]
        for r in &rows { for &id in r { if id != UNBOUND && matches!(id.0 >> 60, 2 | 4 | 0xd | 0xe) { bound_ids.insert(id); } } }
    }
    #[cfg(not(feature = "rdf-12"))]
    let input_labels: HashSet<String> = bound_ids.into_iter()
        .filter_map(|id| match ctx.resolve(id) { Term::BlankNode(b) => Some(b.as_str().to_string()), _ => None })
        .collect();
    #[cfg(feature = "rdf-12")]
    let input_labels: HashSet<String> = {
        let mut labels = HashSet::new();
        for id in bound_ids { triple_term::blank_labels(&ctx.resolve(id), &mut labels) }
        labels
    };
    // A deterministic label per (template label, row). The text after the last `r` is the row number, so base labels
    // are distinct from one another; on a collision, `_<k>` (no `r`, not all digits) keeps it distinct from every
    // base label and from other suffixed ones.
    let fresh = |tpl: &str, n: usize| -> String {
        let base = format!("{tpl}r{n}");
        if !input_labels.contains(&base) { return base }
        (1..).map(|k| format!("{base}_{k}")).find(|l| !input_labels.contains(l)).expect("finite input set")
    };
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (n, r) in rows.iter().enumerate() {
        for t in template {
            let inst = |tp: &TermPattern| -> Option<Term> {
                match tp {
                    TermPattern::BlankNode(b) => Some(BlankNode::new_unchecked(fresh(b.as_str(), n)).into()),
                    // A template triple term `<<( s p o )>>` emits a triple term (SPARQL 1.2).
                    #[cfg(feature = "rdf-12")]
                    TermPattern::Triple(t) => ctx.instantiate_triple(t, r, &|b: &BlankNode| -> Term { BlankNode::new_unchecked(fresh(b.as_str(), n)).into() }),
                    tp => ctx.term_slot(tp, r).and_then(|id| (id != UNBOUND).then(|| ctx.resolve(id))),
                }
            };
            let (Some(s), Some(o)) = (inst(&t.subject), inst(&t.object)) else { continue };
            let p = match &t.predicate {
                NamedNodePattern::NamedNode(n) => Some(Term::NamedNode(n.clone())),
                NamedNodePattern::Variable(v) => { let id = r[ctx.var(v)]; (id != UNBOUND).then(|| ctx.resolve(id)) }
            };
            let (Some(Term::NamedNode(p)), Ok(s)) = (p, triplelight_rdf::NamedOrBlankNode::try_from(s)) else { continue };
            let tr = triplelight_rdf::Triple::new(s, p, o);
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
        let Ok(subject) = triplelight_rdf::NamedOrBlankNode::try_from(ctx.resolve(s)) else { continue };
        let mut triples = Vec::new();
        ctx.scan_each(Perm::Spo, &[s], |_, p, o| triples.push((p, o)));
        for (p, o) in triples {
            let Term::NamedNode(p) = ctx.resolve(p) else { continue };
            let object = ctx.resolve(o);
            if matches!(object, Term::BlankNode(_)) && seen.insert(o) { queue.push(o) }
            out.push(triplelight_rdf::Triple::new(subject.clone(), p, object));
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
    /// `Options::memory_budget`, charged by the operators (`Ctx::fits`, `Ctx::reserve`).
    budget: Option<&'a MemoryBudget>,
    /// `Options::interrupt`, checked by `Ctx::check` and in the long loops.
    interrupt: Option<&'a Interrupt>,
    named: Vec<TermId>,
    /// Under `Entailment::Rdfs`, the entailed view of each named graph of `named` (`eval_graph` reads through it).
    entailed_named: HashMap<TermId, &'a dyn Snapshot>,
    views: &'a Views<'a>,
    vars: HashMap<String, usize>,
    scratch: RefCell<(HashMap<Term, TermId>, Vec<Term>)>,
    stats: Option<(&'a Stats, &'a dyn Snapshot)>,
    wcoj: bool,
    /// `Options::general_group`.
    general_group: bool,
    /// `Options::count_pushdown`.
    count_pushdown: bool,
    /// `Options::range_pushdown`.
    range_pushdown: bool,
    /// `Options::char_pairs`.
    char_pairs: bool,
    /// `Options::sorted_probe`.
    sorted_probe: bool,
    /// CSR adjacencies of single-predicate closures for this query and their policy, including `Options::csr_paths`
    /// (`path.rs`).
    csr: RefCell<path::CsrCache>,
    /// Range pushdown's per-predicate check (`range.rs`), per query, in front of the snapshot's `object_memo`: the
    /// stored-literal objects of a predicate if its literals of that datatype are all canonical, else `None`.
    range_checks: RefCell<HashMap<(TermId, &'static str), Option<std::sync::Arc<[TermId]>>>>,
    /// BGPs (by the address of their pattern list in the query) whose solution order cannot change the answers:
    /// the only ones range pushdown may reorder (`range::reorderable_bgps`).
    range_reorderable: HashSet<usize>,
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
    functions: Option<&'a dyn udf::FunctionLookup>,
    function_memo: Option<RefCell<udf::Memo<'a>>>,
}

impl<'a, S: Snapshot + ?Sized> Ctx<'a, S> {
    /// Initialize a query context over the graph views resolved by the caller.
    fn for_query(
        snap: &'a S,
        dataset: Option<&'a dyn DatasetView>,
        views: &'a Views<'a>,
        named: Vec<TermId>,
        query: &Query,
        opts: &Options<'a>,
        extensions: udf::QueryExtensions<'a>,
        csr_limits: path::CsrLimits,
    ) -> Self
    where
        S: Sized,
    {
        let now = expr::now_lexical();
        let (pattern, spec, base) = match query {
            Query::Select { pattern, dataset, base_iri, .. }
            | Query::Ask { pattern, dataset, base_iri, .. }
            | Query::Construct { pattern, dataset, base_iri, .. }
            | Query::Describe { pattern, dataset, base_iri, .. } => (pattern, dataset, base_iri),
        };
        let mut range_reorderable = HashSet::new();
        // With FROM / FROM NAMED the default graph is not `snap`, so no BGP is marked.
        if opts.range_pushdown && spec.is_none() {
            let nodes = |p: &triplelight_rdf::NamedNode| range::iri_objects(snap, p);
            range::reorderable_bgps(pattern, true, &nodes, &mut range_reorderable);
        }
        Self {
            snap,
            data: std::cell::Cell::new(views.default_graph()),
            dataset,
            named,
            entailed_named: views.named(),
            views,
            interrupt: opts.interrupt,
            budget: opts.memory_budget,
            vars: HashMap::new(),
            scratch: RefCell::new(views.seeded_scratch()),
            stats: opts.stats.map(|stats| (stats, snap as &dyn Snapshot)),
            wcoj: opts.wcoj,
            general_group: opts.general_group,
            count_pushdown: opts.count_pushdown,
            range_pushdown: opts.range_pushdown,
            char_pairs: opts.char_pairs,
            sorted_probe: opts.sorted_probe,
            csr: RefCell::new(path::CsrCache::new(opts.csr_paths, csr_limits)),
            range_checks: RefCell::new(HashMap::new()),
            range_reorderable,
            order_limit: std::cell::Cell::new(None),
            base: base.clone(),
            now,
            exists_memo: RefCell::new(HashMap::new()),
            exists_vars: RefCell::new(HashMap::new()),
            agg_arg_memo: RefCell::new(HashMap::new()),
            agg_arg_slots: RefCell::new(HashMap::new()),
            functions: extensions.functions,
            function_memo: (!extensions.disable_function_memo && extensions.functions.is_some()).then(RefCell::default),
            memo_slots: std::cell::Cell::new(0),
        }
    }
    fn empty_row(&self) -> Row { vec![UNBOUND; self.vars.len()] }
    /// Stops the query here if it was cancelled or is past its deadline (`Interrupt`).
    pub(crate) fn check(&self) { if let Some(i) = self.interrupt { i.check() } }
    /// Estimated heap bytes of one materialized row (`Row`: its header plus one ID per variable).
    pub(crate) fn row_bytes(&self) -> u64 { 24 + 8 * self.vars.len() as u64 }
    /// Ends the query with `ResourceLimit` unless `bytes` more fit in the budget now (a structure about to exist, or one
    /// that exists only while its operator runs). Without a budget, always fits.
    pub(crate) fn fits(&self, bytes: u64) {
        if let Some(b) = self.budget && bytes > b.available() { limits::over_budget(OverBudget { requested: bytes, available: b.available() }) }
    }
    /// Holds `bytes` against the budget until the returned reservation drops (`None` without a budget).
    pub(crate) fn reserve(&self, bytes: u64) -> Option<Reservation<'a>> {
        self.budget.map(|b| b.try_charge(bytes).unwrap_or_else(|e| limits::over_budget(e)))
    }
    /// Grows `r` (from `reserve`) to `total` bytes, or ends the query with `ResourceLimit`.
    pub(crate) fn resize(r: &mut Option<Reservation<'_>>, total: u64) {
        if let Some(r) = r { r.try_resize(total).unwrap_or_else(|e| limits::over_budget(e)) }
    }
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
    fn lookup(&self, t: &Term) -> Option<TermId> {
        if let Some(id) = self.views.virtual_id(t) { return Some(id) }
        self.snap.lookup(t).unwrap_or_else(|e| storage_failure(e))
    }
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
        // Also register variables mentioned only in ORDER BY expressions and aggregate arguments (for example a GRAPH
        // variable inside EXISTS there): they are evaluated but are not in-scope solution variables, so `walk_vars`,
        // which also serves SELECT * visibility, does not report them.
        walk_sort_and_aggregate_exprs(p, &mut names);
        for n in names { self.var_name(&n); }
    }
    fn register_triple(&mut self, t: &TriplePattern) {
        #[cfg(not(feature = "rdf-12"))]
        for tp in [&t.subject, &t.object] { if let TermPattern::Variable(v) = tp { self.var_name(v.as_str()); } }
        // Variables nested in template triple terms too.
        #[cfg(feature = "rdf-12")]
        for tp in [&t.subject, &t.object] {
            let mut names = Vec::new();
            triple_term::term_names(tp, false, &mut |n: String| names.push(n));
            for n in names { self.var_name(&n); }
        }
        if let NamedNodePattern::Variable(v) = &t.predicate { self.var_name(v.as_str()); }
    }
    fn collect_visible(&self, p: &GraphPattern) -> Vec<Variable> {
        let mut names = Vec::new();
        walk_vars(p, &mut names);
        names.into_iter().filter(|n| !crate::sparql::is_internal_variable(n)).map(Variable::new_unchecked).collect()
    }
    /// For CONSTRUCT templates: value of a template slot under solution `row`. Blank nodes and (SPARQL 1.2) triple
    /// terms are instantiated by the caller (`run_construct`, `instantiate_triple`), which owns the fresh labels.
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
        self.check();
        let operator = plan_trace::operator(p, input.len());
        let out = self.eval_op(p, input);
        operator.finish(out.len());
        // Every operator's result is a materialized row set: it must fit next to what the others hold.
        self.fits(out.len() as u64 * self.row_bytes());
        out
    }
    fn eval_op(&self, p: &GraphPattern, input: Vec<Row>) -> Vec<Row> {
        match p {
            GraphPattern::Bgp { patterns } => self.eval_bgp(patterns, input),
            GraphPattern::Join { left, right } => {
                let l = self.eval(left, input);
                // Passing the left solutions into the right group (sideways) is only equivalent to SPARQL's
                // bottom-up join when no FILTER/BIND in the right group reads a variable that group does not bind
                // itself; otherwise evaluate the right group on its own and join the solution sets.
                if scope_safe(right) { self.eval(right, l) } else {
                    let r = self.eval(right, vec![self.empty_row()]);
                    phase::join(|| self.join_within_budget(l, r))
                }
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
                phase::join(|| self.join_within_budget(input, projected))
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
            let merged = match &standalone { Some(r) => phase::join(|| join_rows(vec![row.clone()], r.clone())), None => self.eval(right, vec![row.clone()]) };
            let ext: Vec<Row> = phase::filter(|| merged.into_iter().filter(|r| expression.as_ref().is_none_or(|e| self.ebv(e, r))).collect());
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
                return phase::filter(|| rows.into_iter().map(|mut r| { r[bi] = r[ai]; r }).collect());
            }
            let (early, late) = self.pushed_filters(expr, inner);
            if !early.is_empty() {
                // Pushdown bypasses `eval(inner)`, so install the BGP's observed-tree node explicitly.
                let bgp_operator = plan_trace::operator(inner, input.len());
                let rows = self.eval_bgp_filtered(patterns, input, &early);
                bgp_operator.finish(rows.len());
                if late.is_empty() { return rows }
                return phase::filter(|| rows.into_iter().filter(|r| late.iter().all(|c| self.ebv(c, r))).collect());
            }
        }
        let rows = self.eval(inner, input);
        if let Some(out) = self.exists_batch(expr, &rows) { return out }
        // Numeric comparisons (a HAVING over aggregate results, #121) on numbers, each distinct ID decoded once; a row
        // the numeric path cannot evaluate takes `ebv`.
        let fast = expr::NumFilter::compile(expr, &self.vars);
        let mut nums: HashMap<TermId, Option<expr::Num>> = HashMap::new();
        let mut val = |id: TermId| *nums.entry(id).or_insert_with(|| expr::term_num(&self.resolve(id)));
        phase::filter(|| rows.into_iter().filter(|r| match fast.as_ref().map(|f| f.eval(r, &mut val)) {
            Some(Ok(b)) => b == Some(true),
            _ => self.ebv(expr, r),
        }).collect())
    }

    /// The conjuncts of `expr` over `inner`'s BGP split into those applied inside the BGP (`eval_bgp_filtered`), with
    /// their variables, and those left for after it.
    fn pushed_filters<'e>(&self, expr: &'e Expression, inner: &GraphPattern) -> (Vec<(&'e Expression, Vec<usize>)>, Vec<&'e Expression>) {
        let bgp_vars: HashSet<usize> = { let mut v = Vec::new(); walk_vars(inner, &mut v); v.iter().filter_map(|n| self.vars.get(n).copied()).collect() };
        let (early, late): (Vec<&Expression>, Vec<&Expression>) = conjuncts(expr).into_iter().partition(|c| {
            let vs = expr_vars(c);
            !vs.is_empty() && !impure(c) && vs.iter().all(|n| self.vars.get(n).is_some_and(|i| bgp_vars.contains(i)))
        });
        (early.into_iter().map(|c| (c, expr_vars(c).iter().map(|n| self.vars[n]).collect())).collect(), late)
    }

    fn eval_extend(&self, inner: &GraphPattern, variable: &Variable, expression: &Expression, input: Vec<Row>) -> Vec<Row> {
        if expr::extend::has_named_blank(expression) {
            return self.eval_scoped_extend(inner, variable, expression, input);
        }
        let i = self.var(variable);
        // If an outer solution passed in sideways already binds the target, the BIND result must be
        // compatible with it (the group's solution is joined with the outer one): equal keeps the row, a
        // different value drops it, an expression error leaves the outer value (W3C-style `{ BIND(2 AS ?x) }`).
        // Arithmetic and `xsd:float` over numbers (#121) skip the per-row `Term`: each distinct input ID is decoded once,
        // the result is a number, and only distinct results become terms (`intern_num`). A row the numeric path cannot
        // evaluate (a non-numeric input) takes the general evaluator, so the values are those of `value` either way.
        let fast = expr::NumExpr::compile_value(expression, &self.vars);
        let mut nums: HashMap<TermId, Option<expr::Num>> = HashMap::new();
        let mut val = |id: TermId| *nums.entry(id).or_insert_with(|| expr::term_num(&self.resolve(id)));
        let mut results: HashMap<expr::NumKey, TermId> = HashMap::new();
        let rows = self.eval(inner, input);
        phase::extend(|| rows.into_iter().filter_map(|mut r| {
            let id = match fast.as_ref().map(|f| f.value(&r, &mut val)) {
                Some(Ok(Some(n))) => Some(*results.entry(expr::NumKey::of(n)).or_insert_with(|| self.intern(&n.term()))),
                Some(Ok(None)) => None,
                _ => self.value(expression, &r).map(|t| self.intern(&t)),
            };
            if let Some(id) = id {
                if r[i] == UNBOUND { r[i] = id } else if r[i] != id { return None }
            }
            Some(r)
        }).collect())
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
                    let id = self.intern(&ground_to_term(v));
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
        let alias = self.order_alias(inner, expression, &input);
        let rows = self.eval(inner, input);
        phase::sort(|| self.sort_rows(expression, limit, rows, alias))
    }

    /// A fresh alias computed immediately below ORDER BY can supply the same pure expression.
    fn order_alias<'p>(
        &self,
        inner: &'p GraphPattern,
        keys: &[OrderExpression],
        input: &[Row],
    ) -> Option<(&'p Expression, usize)> {
        let GraphPattern::Extend { inner, variable, expression } = inner else { return None };
        if !keys.iter().any(|key| match key {
            OrderExpression::Asc(e) | OrderExpression::Desc(e) => e == expression,
        }) || impure(expression) {
            return None;
        }
        let col = self.var(variable);
        if input.iter().any(|row| row[col] != UNBOUND) {
            return None;
        }
        let mut names = Vec::new();
        walk_vars(inner, &mut names);
        walk_expr(expression, &mut names);
        // If the expression errors, Extend leaves this fresh column unbound. The operand columns
        // are unchanged, so ordering by the alias reproduces both values and expression errors.
        (!names.iter().any(|name| name == variable.as_str())).then_some((expression, col))
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
        let view = |g: TermId| match self.entailed_named.get(&g) {
            Some(&e) => Some(e),
            None => ds.graph(g).unwrap_or_else(|e| storage_failure(e)),
        };
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
        if self.count_pushdown && let Some(rows) = phase::count(|| self.try_count_group(inner, keys, aggs, &input)) { return rows; }
        let input = match self.try_flat_count_group(inner, keys, aggs, input) { Ok(rows) => return rows, Err(input) => input };
        let rows = self.eval(inner, input);
        phase::group(|| self.group_rows(keys, aggs, rows))
    }

    fn group_rows(&self, keys: &[Variable], aggs: &[(Variable, AggregateExpression)], rows: Vec<Row>) -> Vec<Row> {
        let kidx: Vec<usize> = keys.iter().map(|v| self.var(v)).collect();
        // Over the budget, GROUP BY works one key partition at a time from disk (#111; `agg/spilled.rs`). The fast
        // paths borrow their input, so they try memory first; the general path decides up front.
        let forced = self.forced_spill(rows.len());
        if !self.general_group && !forced {
            match self.in_memory_or_spill(!kidx.is_empty(), || self.try_fast_group(&rows, &kidx, aggs)) {
                Some(Some(out)) => return out,
                Some(None) => {}
                None => return self.group_spilled(keys, aggs, &kidx, rows),
            }
        }
        if !kidx.is_empty() && self.should_spill_general(rows.len()) { return self.group_spilled(keys, aggs, &kidx, rows) }
        self.group_general(keys, aggs, &kidx, rows)
    }

    fn group_spilled(&self, keys: &[Variable], aggs: &[(Variable, AggregateExpression)], kidx: &[usize], rows: Vec<Row>) -> Vec<Row> {
        let spilled = self.spill_for_group(kidx, rows.len(), rows.iter().map(Vec::as_slice));
        drop(rows);
        self.group_partitions(&spilled, kidx, |part| {
            if !self.general_group && let Some(out) = self.try_fast_group(&part, kidx, aggs) { return out }
            self.group_general(keys, aggs, kidx, part)
        })
    }

    fn group_general(&self, keys: &[Variable], aggs: &[(Variable, AggregateExpression)], kidx: &[usize], rows: Vec<Row>) -> Vec<Row> {
        let mut groups: Vec<(Vec<TermId>, Vec<Row>)> = Vec::new();
        let mut pos: HashMap<Vec<TermId>, usize> = HashMap::new();
        // The groups hold every row again, plus a key and a table entry per group.
        let mut held = self.reserve(rows.len() as u64 * self.row_bytes());
        for (n, r) in rows.into_iter().enumerate() {
            if n.is_multiple_of(4096) { Self::resize(&mut held, (n as u64 + groups.len() as u64) * self.row_bytes() + groups.len() as u64 * 64) }
            if n.is_multiple_of(4096) { self.check() }
            let k: Vec<TermId> = kidx.iter().map(|&i| r[i]).collect();
            let g = *pos.entry(k.clone()).or_insert_with(|| { groups.push((k, Vec::new())); groups.len() - 1 });
            groups[g].1.push(r);
        }
        if groups.is_empty() && keys.is_empty() { groups.push((vec![], vec![])); }
        let parallel = self.try_parallel_expression_group(&groups, aggs);
        groups.into_iter().enumerate().map(|(group_index, (k, members))| {
            let mut out = self.empty_row();
            for (i, &ki) in kidx.iter().enumerate() { out[ki] = k[i]; }
            if let Some((col, values)) = &parallel {
                if let Some(value) = values[group_index] { out[*col] = self.intern(&value.term()); }
            } else {
                for (v, agg) in aggs {
                    if let Some(t) = self.aggregate(agg, &members) { out[self.var(v)] = self.intern(&t); }
                }
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
            // A ground triple term is a constant; any other stands for its hidden variable, unified afterwards
            // (`triple_term::eval_bgp_triple_terms`).
            #[cfg(feature = "rdf-12")]
            TermPattern::Triple(t) => match triple_term::hidden_var(t) {
                Some(h) => Slot::Var(self.vars[&h]),
                None => Slot::Const(triple_term::ground_term(t).and_then(|term| self.lookup(&term))),
            },
        }
    }

    fn eval_bgp(&self, patterns: &[TriplePattern], rows: Vec<Row>) -> Vec<Row> { self.eval_bgp_filtered(patterns, rows, &[]) }

    /// BGP evaluation that applies each filter in `filters` as soon as all its variables are bound.
    /// Timed as the `bgp` phase (S6): every BGP evaluation, with or without pushed-down FILTER conjuncts, goes through here.
    fn eval_bgp_filtered(&self, patterns: &[TriplePattern], rows: Vec<Row>, filters: &[(&Expression, Vec<usize>)]) -> Vec<Row> {
        phase::bgp(|| self.eval_bgp_filtered_untimed(patterns, rows, filters))
    }

    /// `eval_bgp_filtered` as one flat buffer of rows, for a BGP with at least one variable and no triple terms.
    fn eval_bgp_filtered_flat(&self, patterns: &[TriplePattern], rows: Vec<Row>, filters: &[(&Expression, Vec<usize>)]) -> Vec<TermId> {
        phase::bgp(|| self.eval_bgp_flat(patterns, rows, filters))
    }

    fn eval_bgp_filtered_untimed(&self, patterns: &[TriplePattern], rows: Vec<Row>, filters: &[(&Expression, Vec<usize>)]) -> Vec<Row> {
        #[cfg(feature = "rdf-12")]
        if patterns.iter().any(triple_term::has_triple_term) { return self.eval_bgp_triple_terms(patterns, rows, filters) }
        self.eval_bgp_slots(patterns, rows, filters)
    }

    /// `eval_bgp_filtered` over patterns whose terms are all plain slots (variables and constants).
    fn eval_bgp_slots(&self, patterns: &[TriplePattern], rows: Vec<Row>, filters: &[(&Expression, Vec<usize>)]) -> Vec<Row> {
        let w = self.vars.len();
        if w == 0 {
            // No variables anywhere: every pattern is a membership test.
            let tr = plan_trace::enabled().then(|| plan_trace::Trace::new(patterns, rows.len()));
            let all = !rows.is_empty() && patterns.iter().all(|t| {
                let slots = self.slots(t);
                // A constant absent from the dictionary can never match.
                if slots.iter().any(|s| matches!(s, Slot::Const(None))) { return false }
                let ids: Vec<TermId> = slots.iter().map(|s| if let Slot::Const(Some(c)) = s { *c } else { UNBOUND }).collect();
                self.count(Perm::Spo, &ids) > 0
            });
            let out = if all { rows } else { Vec::new() };
            if let Some(tr) = tr { tr.finish("ground", out.len()) }
            return out;
        }
        self.eval_bgp_flat(patterns, rows, filters).chunks_exact(w).map(<[TermId]>::to_vec).collect()
    }

    fn slots(&self, t: &TriplePattern) -> [Slot; 3] {
        let p = match &t.predicate {
            NamedNodePattern::Variable(v) => Slot::Var(self.var(v)),
            NamedNodePattern::NamedNode(n) => Slot::Const(self.lookup(&n.clone().into())),
        };
        [self.slot_of(&t.subject), p, self.slot_of(&t.object)]
    }

    /// `eval_bgp_slots` for a query with at least one variable, as one flat buffer of rows (`vars.len()` IDs each),
    /// so a consumer that reads the rows once (`agg::flat`) skips building a `Row` per solution.
    fn eval_bgp_flat(&self, patterns: &[TriplePattern], rows: Vec<Row>, filters: &[(&Expression, Vec<usize>)]) -> Vec<TermId> {
        let pats: Vec<[Slot; 3]> = patterns.iter().map(|t| self.slots(t)).collect();
        let mut tr = plan_trace::enabled().then(|| plan_trace::Trace::new(patterns, rows.len()));
        // A constant absent from the dictionary can never match.
        if rows.is_empty() || pats.iter().any(|p| p.iter().any(|s| matches!(s, Slot::Const(None)))) {
            if let Some(tr) = tr { tr.finish("empty", 0) }
            return Vec::new();
        }
        let w = self.vars.len();
        let unbound_start = rows.len() == 1 && rows[0].iter().all(|&x| x == UNBOUND);
        let ranges = self.object_ranges(patterns, &pats, filters, &rows[0]);
        let flat: Vec<TermId> = rows.concat();
        // Numeric literals often flow through multiple range predicates in the same BGP. Keep decoded
        // values for the whole BGP instead of resolving and parsing them again for every predicate.
        let mut numeric_filter_memo: id_hash::IdMap<Option<expr::Num>> = id_hash::IdMap::default();
        if let Some(tr) = tr.as_mut() { tr.note("ranges", ranges.len()); tr.note("filters", filters.len()) }
        let (out, strategy) = on_data!(self, |g| {
            let bgp = bgp::Bgp { snap: g, w, ranges: (!ranges.is_empty()).then_some(&ranges), sorted_probe: self.sorted_probe, interrupt: self.interrupt, budget: self.budget };
            if self.wcoj && unbound_start && pats.len() >= 3 && pats.iter().all(pattern::no_repeated_var) && pattern::cyclic(&pats) {
                let given = self.variable_order(&pats);
                let t = tr.as_mut().map(|tr| {
                    tr.note("var_order", given.as_deref().map_or_else(|| "default".to_string(), |o| plan_trace::var_names(o, &self.vars)));
                    std::time::Instant::now()
                });
                let mut out = bgp.leapfrog(&pats, &flat, given);
                let joined = out.len() / w;
                if !filters.is_empty() { let all: HashSet<usize> = (0..w).collect(); self.apply_filters(&mut out, w, filters, &all, &mut vec![false; filters.len()], &mut numeric_filter_memo) }
                if let (Some(tr), Some(t)) = (tr.as_mut(), t) { tr.step(None, "leapfrog", 1, joined, out.len() / w, t) }
                (out, "leapfrog")
            } else {
                let mut applied = vec![false; filters.len()];
                let mut post = |rows: &mut Vec<TermId>, bound: &HashSet<usize>| self.apply_filters(rows, w, filters, bound, &mut applied, &mut numeric_filter_memo);
                match self.planned_order_est(&pats, &rows, &ranges, tr.is_some()) {
                    Some((order, est)) => {
                        if let Some(tr) = tr.as_mut() { tr.planned(&order, est) }
                        (bgp.eval_order(&pats, &order, flat, &mut post, tr.as_mut()), "planner")
                    }
                    None => (bgp.eval_greedy(&pats, flat, &mut post, tr.as_mut()), "greedy"),
                }
            }
        });
        if let Some(tr) = tr { tr.finish(strategy, out.len() / w) }
        out
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
        // Renaming would change the hidden variables of triple-term patterns (named by the pattern text), which are
        // registered before evaluation; such BGPs keep the general path.
        #[cfg(feature = "rdf-12")]
        if patterns.iter().any(triple_term::has_triple_term) { return None }
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
    fn planned_order(&self, pats: &[[Slot; 3]], rows: &[Row], ranges: &range::Ranges) -> Option<Vec<usize>> {
        self.planned_order_est(pats, rows, ranges, false).map(|(order, _)| order)
    }

    /// `planned_order`, with (if `want_est`, for `plan_trace`) the planner's estimate per pattern index: the estimated
    /// rows of the subplan whose last leaf is that pattern.
    fn planned_order_est(&self, pats: &[[Slot; 3]], rows: &[Row], ranges: &range::Ranges, want_est: bool) -> Option<(Vec<usize>, Option<Vec<f64>>)> {
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
        st.set_char_pairs(self.char_pairs);
        let clone_ns = elapsed(phase);
        let phase = clock();
        st.cache_pattern_counts(&tps, dynsnap).unwrap_or_else(|e| storage_failure(std::io::Error::other(e)));
        // A ranged pattern scans only its range's rows (`bgp::Bgp::extend_range`): plan with that count.
        for (p, tp) in pats.iter().zip(&tps) {
            if let [Slot::Var(_), Slot::Const(Some(pid)), Slot::Var(o)] = *p && let Some(r) = ranges.get(&(pid, o)) {
                st.set_exact_count(*tp, r.rows);
            }
        }
        let prefix_ns = elapsed(phase);
        let phase = clock();
        let tree = opt::planner::plan_bgp(&tps, &st);
        let search_ns = elapsed(phase);
        // The executor runs the leaves in order as index nested-loop joins. A merge join's two sides tie in cost
        // whichever is left, so with a pushed-down range the smaller side goes first (as a hash join's build side does).
        // Each leaf carries the estimate of the subplan it completes (a join node's estimate goes to its last leaf).
        fn leaves(t: &opt::plan::JoinTree, smaller_first: bool, out: &mut Vec<(opt::plan::TriplePattern, f64)>) {
            use opt::plan::JoinTree::*;
            match t {
                Scan { pattern, est, .. } => out.push((*pattern, *est)),
                IndexNestedLoop { outer, pattern, est, .. } => { leaves(outer, smaller_first, out); out.push((*pattern, *est)); }
                MergeJoin { left, right, .. } if smaller_first && right.est() < left.est() => { leaves(right, smaller_first, out); leaves(left, smaller_first, out) }
                MergeJoin { left, right, .. } => { leaves(left, smaller_first, out); leaves(right, smaller_first, out) }
                HashJoin { build, probe, .. } => { leaves(build, smaller_first, out); leaves(probe, smaller_first, out) }
                Leapfrog { children, .. } => for c in children { leaves(c, smaller_first, out) },
                Unit | Empty => {}
            }
            if matches!(t, MergeJoin { .. } | HashJoin { .. } | Leapfrog { .. }) && let Some(last) = out.last_mut() { last.1 = t.est() }
        }
        let mut ls = Vec::new();
        leaves(&tree, !ranges.is_empty(), &mut ls);
        let mut used = vec![false; tps.len()];
        let order: Vec<usize> = ls.iter().filter_map(|l| {
            let i = (0..tps.len()).find(|&i| !used[i] && tps[i] == l.0)?; used[i] = true; Some(i)
        }).collect();
        if trace { eprintln!("planning_profile clone_ns={clone_ns} prefix_ns={prefix_ns} search_ns={search_ns} gate_ns={gate_ns} total_ns={}", elapsed(total_clock)); }
        if order.len() != tps.len() { return None }
        let est = want_est.then(|| {
            let mut est = vec![f64::NAN; tps.len()];
            let mut used = vec![false; tps.len()];
            for l in &ls {
                if let Some(i) = (0..tps.len()).find(|&i| !used[i] && tps[i] == l.0) { used[i] = true; est[i] = l.1 }
            }
            est
        });
        Some((order, est))
    }

    pub(crate) fn ebv(&self, e: &Expression, r: &Row) -> bool {
        self.value(e, r).and_then(|t| expr::ebv(&t)).unwrap_or(false)
    }
    pub(crate) fn value(&self, e: &Expression, r: &Row) -> Option<Term> {
        self.value_scoped(e, r, &expr::BlankScope::default())
    }
    fn value_scoped(&self, e: &Expression, r: &Row, blanks: &expr::BlankScope) -> Option<Term> {
        expr::eval(
            e,
            &|variable| self.var_value(variable, r),
            &|pattern| self.exists(pattern, r),
            &|iri, args, evaluate| self.call_scalar(iri, args, evaluate),
            self.base.as_deref(),
            &self.now,
            blanks,
        )
    }
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
        #[cfg(not(feature = "rdf-12"))]
        GraphPattern::Path { subject, object, .. } => [subject, object].into_iter().filter_map(|t| match t {
            TermPattern::Variable(v) => Some(v.as_str().to_string()),
            _ => None,
        }).collect(),
        // Variables nested in a triple-term endpoint are bound by its unification too.
        #[cfg(feature = "rdf-12")]
        GraphPattern::Path { subject, object, .. } => {
            let mut b = HashSet::new();
            for t in [subject, object] { triple_term::term_names(t, false, &mut |n: String| { b.insert(n); }) }
            b
        }
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
        | GraphPattern::Project { inner, .. } => memo_safe(inner),
        GraphPattern::Group { inner, aggregates, .. } => memo_safe(inner) && aggregates.iter().all(|(_, aggregate)| match aggregate {
            AggregateExpression::CountSolutions { .. } => true,
            AggregateExpression::FunctionCall { expr, .. } => !impure(expr),
        }),
    }
}

/// No pattern has a triple term (always so without `rdf-12`), so `eval_bgp_slots` evaluates the BGP.
fn triple_term_free(patterns: &[TriplePattern]) -> bool {
    #[cfg(feature = "rdf-12")]
    if patterns.iter().any(triple_term::has_triple_term) { return false }
    let _ = patterns;
    true
}

fn walk_vars(p: &GraphPattern, out: &mut Vec<String>) {
    let mut add = |s: String| if !out.contains(&s) { out.push(s) };
    match p {
        GraphPattern::Bgp { patterns } => for t in patterns {
            for tp in [&t.subject, &t.object] {
                match tp {
                    TermPattern::Variable(v) => add(v.as_str().to_string()),
                    TermPattern::BlankNode(b) => add(format!("_:b{}", b.as_str())),
                    // Variables and blank nodes nested in a triple-term pattern (SPARQL 1.2 §18.3.1), then the hidden
                    // variable of its slot.
                    #[cfg(feature = "rdf-12")]
                    TermPattern::Triple(tt) => {
                        triple_term::term_names(tp, true, &mut add);
                        if let Some(h) = triple_term::hidden_var(tt) { add(h) }
                    }
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
                #[cfg(feature = "rdf-12")]
                TermPattern::Triple(tt) => {
                    triple_term::term_names(tp, true, &mut add);
                    if let Some(h) = triple_term::hidden_var(tt) { add(h) }
                }
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

/// Variables mentioned in ORDER BY expressions and aggregate arguments anywhere in `p`, including inside EXISTS
/// patterns nested in any expression (for registration only).
fn walk_sort_and_aggregate_exprs(p: &GraphPattern, out: &mut Vec<String>) {
    match p {
        GraphPattern::OrderBy { inner, expression } => {
            for o in expression {
                let (OrderExpression::Asc(e) | OrderExpression::Desc(e)) = o;
                walk_expr(e, out);
                sort_and_aggregate_in_expr(e, out);
            }
            walk_sort_and_aggregate_exprs(inner, out)
        }
        GraphPattern::Group { inner, aggregates, .. } => {
            for (_, a) in aggregates {
                if let AggregateExpression::FunctionCall { expr, .. } = a { walk_expr(expr, out); sort_and_aggregate_in_expr(expr, out) }
            }
            walk_sort_and_aggregate_exprs(inner, out)
        }
        GraphPattern::LeftJoin { left, right, expression } => {
            if let Some(e) = expression { sort_and_aggregate_in_expr(e, out) }
            walk_sort_and_aggregate_exprs(left, out); walk_sort_and_aggregate_exprs(right, out)
        }
        GraphPattern::Filter { expr, inner } => { sort_and_aggregate_in_expr(expr, out); walk_sort_and_aggregate_exprs(inner, out) }
        GraphPattern::Extend { inner, expression, .. } => { sort_and_aggregate_in_expr(expression, out); walk_sort_and_aggregate_exprs(inner, out) }
        GraphPattern::Join { left, right } | GraphPattern::Union { left, right } | GraphPattern::Minus { left, right }
        => { walk_sort_and_aggregate_exprs(left, out); walk_sort_and_aggregate_exprs(right, out) }
        GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner } | GraphPattern::Slice { inner, .. }
        | GraphPattern::Service { inner, .. } | GraphPattern::Graph { inner, .. } | GraphPattern::Project { inner, .. }
        => walk_sort_and_aggregate_exprs(inner, out),
        GraphPattern::Bgp { .. } | GraphPattern::Path { .. } | GraphPattern::Values { .. } => {}
    }
}

/// Follows EXISTS patterns inside `e` into `walk_sort_and_aggregate_exprs`.
fn sort_and_aggregate_in_expr(e: &Expression, out: &mut Vec<String>) {
    match e {
        Expression::Exists(p) => walk_sort_and_aggregate_exprs(p, out),
        e => for c in e.children() { sort_and_aggregate_in_expr(c, out) },
    }
}

fn walk_expr(e: &Expression, out: &mut Vec<String>) {
    match e {
        Expression::Exists(p) => walk_vars(p, out),
        Expression::Variable(v) | Expression::Bound(v) => { let s = v.as_str().to_string(); if !out.contains(&s) { out.push(s) } }
        e => for c in e.children() { walk_expr(c, out) },
    }
}

/// Superseded by `parse_update_operations` + `operation_changes` (task #36), which add named graphs, sequential
/// operation semantics and fresh blank nodes; kept for API compatibility.
///
/// The change set of a SPARQL 1.1 Update request, computed against `snap` (task #8). Supports INSERT DATA,
/// DELETE DATA and DELETE/INSERT ... WHERE on the default graph. Every operation's WHERE clause is evaluated on
/// `snap` (the state before the request); deletions of an operation precede its insertions.
pub fn update_changes<S: Snapshot>(snap: &S, text: &str) -> Result<Vec<(crate::store::Change, triplelight_rdf::Triple)>, QueryError> {
    use crate::store::Change;
    use triplelight_rdf::{NamedOrBlankNode, Triple};
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
                out.push((Change::Delete, Triple::new(q.subject.clone(), q.predicate.clone(), ground_to_term(&q.object))));
            },
            Op::DeleteInsert { delete, insert, with, using, pattern } => {
                if using.is_some() { return Err(unsupported("USING")) }
                if with.is_some() { return Err(unsupported("WITH (named graphs)")) }
                if delete.iter().any(|d| !matches!(d.graph_name, GraphNamePattern::DefaultGraph))
                    || insert.iter().any(|d| !matches!(d.graph_name, GraphNamePattern::DefaultGraph)) { return Err(unsupported("named graphs")) }
                let q = Query::Select { dataset: None, pattern: (**pattern).clone(), base_iri: None };
                let QueryResult::Solutions { vars, rows } = catching_storage(|| run_query(snap, None, &q, &Options::default(), udf::QueryExtensions::default()))? else { unreachable!() };
                let col: HashMap<&str, usize> = vars.iter().enumerate().map(|(i, v)| (v.as_str(), i)).collect();
                for (n, row) in rows.iter().enumerate() {
                    let var = |v: &Variable| col.get(v.as_str()).and_then(|&i| row[i].clone());
                    let ground = |t: &GroundTermPattern| -> Option<Term> { match t {
                        GroundTermPattern::NamedNode(x) => Some(x.clone().into()),
                        GroundTermPattern::Literal(x) => Some(x.clone().into()),
                        GroundTermPattern::Variable(v) => var(v),
                        #[cfg(feature = "rdf-12")]
                        GroundTermPattern::Triple(t) => ground_triple_template(t, &var),
                    } };
                    let term = |t: &TermPattern| -> Option<Term> { match t {
                        TermPattern::NamedNode(x) => Some(x.clone().into()),
                        TermPattern::Literal(x) => Some(x.clone().into()),
                        TermPattern::BlankNode(b) => Some(BlankNode::new_unchecked(format!("{}u{n}", b.as_str())).into()),
                        TermPattern::Variable(v) => var(v),
                        #[cfg(feature = "rdf-12")]
                        TermPattern::Triple(t) => triple_template(t, &var, &|b: &BlankNode| BlankNode::new_unchecked(format!("{}u{n}", b.as_str()))),
                    } };
                    let pred = |p: &NamedNodePattern| -> Option<triplelight_rdf::NamedNode> { match p {
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
            Op::Load { silent: true, .. } => {}
            _ => return Err(unsupported("graph management")),
        }
    }
    Ok(out)
}

/// The operations of a SPARQL 1.1 Update request, in order (task #36).
pub fn parse_update_operations(text: &str) -> Result<Vec<crate::sparql::GraphUpdateOperation>, QueryError> {
    Ok(crate::sparql::parse_update(text, None).map_err(|e| QueryError::new(QueryErrorKind::Syntax, e.to_string()))?.operations)
}

/// The dataset changes of one update operation, evaluated on `snap` with `dataset` as its named graphs (task #36).
/// The caller applies each operation's changes before computing the next one, so every operation sees the effects of
/// the ones before it (SPARQL 1.1 Update semantics), and commits the concatenated list atomically.
/// Supports INSERT DATA, DELETE DATA, DELETE/INSERT ... WHERE with GRAPH templates, `WITH` and `USING`, and graph
/// management (CREATE, DROP, CLEAR, ADD, MOVE, COPY). Without a source resolver, LOAD reports an error and
/// LOAD SILENT leaves the dataset unchanged.
pub fn operation_changes<S: Snapshot>(snap: &S, dataset: &dyn DatasetView, op: &crate::sparql::GraphUpdateOperation) -> Result<Vec<crate::model::DatasetChange>, QueryError> {
    use crate::model::{Change, DatasetChange, GraphScope, GraphTarget};
    use crate::sparql::{ClearTarget, GraphName, GraphNamePattern, GroundTermPattern, TransferKind};
    use crate::sparql::GraphUpdateOperation as Op;
    use triplelight_rdf::{NamedOrBlankNode, Triple};
    let unsupported = |what: &str| QueryError::new(QueryErrorKind::Unsupported, format!("{what} is not supported yet"));
    let target = |g: &GraphName| match g { GraphName::DefaultGraph => GraphTarget::Default, GraphName::NamedNode(n) => GraphTarget::Named(n.clone()) };
    let scope_of = |g: &ClearTarget| match g {
        ClearTarget::NamedNode(n) => GraphScope::Graph(n.clone()),
        ClearTarget::DefaultGraph => GraphScope::Default,
        ClearTarget::NamedGraphs => GraphScope::Named,
        ClearTarget::AllGraphs => GraphScope::All,
    };
    // Blank nodes in INSERT DATA and INSERT templates are fresh for every operation (and every template solution):
    // a label is scoped to one operation, so running the same update twice creates new nodes. Occurrences of one
    // label within that scope, across graphs too, stay one node.
    let scope = format!("{:016x}{:016x}", expr::random_u64(), expr::random_u64());
    let fresh = |b: &BlankNode, row: Option<usize>| -> BlankNode {
        BlankNode::new_unchecked(match row { Some(n) => format!("{}_{scope}r{n}", b.as_str()), None => format!("{}_{scope}", b.as_str()) })
    };
    let mut out = Vec::new();
    match op {
        Op::InsertData { data } => for q in data {
            let subject = match &q.subject { NamedOrBlankNode::BlankNode(b) => fresh(b, None).into(), s => s.clone() };
            let object = map_blank_nodes(q.object.clone(), &mut |b: &BlankNode| fresh(b, None));
            out.push(DatasetChange::Quad(target(&q.graph_name), Change::Insert, Triple::new(subject, q.predicate.clone(), object)));
        },
        Op::DeleteData { data } => for q in data {
            out.push(DatasetChange::Quad(target(&q.graph_name), Change::Delete, Triple::new(q.subject.clone(), q.predicate.clone(), ground_to_term(&q.object))));
        },
        Op::DeleteInsert { delete, insert, with, using, pattern } => {
            // WITH <g> makes <g> the default graph of the WHERE clause (GRAPH patterns inside it still range over the
            // named graphs) and the target of templates without a GRAPH. USING / USING NAMED replace the dataset.
            let pattern = match (with, using) {
                (Some(g), None) => GraphPattern::Graph { name: NamedNodePattern::NamedNode(g.clone()), inner: pattern.clone() },
                _ => (**pattern).clone(),
            };
            let q = Query::Select { dataset: using.clone(), pattern, base_iri: None };
            let QueryResult::Solutions { vars, rows } = catching_storage(|| run_query(snap, Some(dataset), &q, &Options::default(), udf::QueryExtensions::default()))? else { unreachable!() };
            let col: HashMap<&str, usize> = vars.iter().enumerate().map(|(i, v)| (v.as_str(), i)).collect();
            let default_target = || with.as_ref().map_or(GraphTarget::Default, |g| GraphTarget::Named(g.clone()));
            for (n, row) in rows.iter().enumerate() {
                let var = |v: &Variable| col.get(v.as_str()).and_then(|&i| row[i].clone());
                let ground = |t: &GroundTermPattern| -> Option<Term> { match t {
                    GroundTermPattern::NamedNode(x) => Some(x.clone().into()),
                    GroundTermPattern::Literal(x) => Some(x.clone().into()),
                    GroundTermPattern::Variable(v) => var(v),
                    #[cfg(feature = "rdf-12")]
                    GroundTermPattern::Triple(t) => ground_triple_template(t, &var),
                } };
                let term = |t: &TermPattern| -> Option<Term> { match t {
                    TermPattern::NamedNode(x) => Some(x.clone().into()),
                    TermPattern::Literal(x) => Some(x.clone().into()),
                    TermPattern::BlankNode(b) => Some(fresh(b, Some(n)).into()),
                    TermPattern::Variable(v) => var(v),
                    #[cfg(feature = "rdf-12")]
                    TermPattern::Triple(t) => triple_template(t, &var, &|b: &BlankNode| fresh(b, Some(n))),
                } };
                let pred = |p: &NamedNodePattern| -> Option<triplelight_rdf::NamedNode> { match p {
                    NamedNodePattern::NamedNode(x) => Some(x.clone()),
                    NamedNodePattern::Variable(v) => match var(v) { Some(Term::NamedNode(x)) => Some(x), _ => None },
                } };
                // A template's graph: its GRAPH name, a bound IRI variable, or the WITH / default graph.
                let graph = |g: &GraphNamePattern| -> Option<GraphTarget> { match g {
                    GraphNamePattern::DefaultGraph => Some(default_target()),
                    GraphNamePattern::NamedNode(x) => Some(GraphTarget::Named(x.clone())),
                    GraphNamePattern::Variable(v) => match var(v) { Some(Term::NamedNode(x)) => Some(GraphTarget::Named(x)), _ => None },
                } };
                // Templates with an unbound or ill-typed slot produce no quad for this solution.
                for d in delete {
                    if let (Some(s), Some(p), Some(o), Some(g)) = (ground(&d.subject), pred(&d.predicate), ground(&d.object), graph(&d.graph_name))
                        && let Ok(s) = NamedOrBlankNode::try_from(s) { out.push(DatasetChange::Quad(g, Change::Delete, Triple::new(s, p, o))) }
                }
                for i in insert {
                    if let (Some(s), Some(p), Some(o), Some(g)) = (term(&i.subject), pred(&i.predicate), term(&i.object), graph(&i.graph_name))
                        && let Ok(s) = NamedOrBlankNode::try_from(s) { out.push(DatasetChange::Quad(g, Change::Insert, Triple::new(s, p, o))) }
                }
            }
            // Deletions of an operation precede its insertions.
            out.sort_by_key(|c| !matches!(c, DatasetChange::Quad(_, Change::Delete, _)));
        }
        Op::Load { silent: true, .. } => {}
        Op::Load { .. } => return Err(unsupported("LOAD")),
        Op::Create { silent, graph } => out.push(DatasetChange::Create { graph: graph.clone(), silent: *silent }),
        Op::Clear { silent, graph } => out.push(DatasetChange::Clear { target: scope_of(graph), silent: *silent }),
        Op::Drop { silent, graph } => out.push(DatasetChange::Drop { target: scope_of(graph), silent: *silent }),
        Op::Transfer { kind, silent, from, to } => {
            // SPARQL 1.1 Update §3.2.3-3.2.5: an absent source graph fails the request unless SILENT (and then nothing
            // happens); a transfer from a graph to itself does nothing. COPY and MOVE replace the destination, ADD
            // keeps it; MOVE then drops the source. The default graph always exists. The self-transfer check comes first,
            // so a transfer of an absent graph to itself is a no-op too.
            if from == to { return Ok(out) }
            if let GraphName::NamedNode(g) = from && !graph_exists(snap, dataset, g)? {
                if *silent { return Ok(out) }
                return Err(QueryError::new(QueryErrorKind::GraphState, format!("graph {g} does not exist")));
            }
            let triples = graph_triples(snap, dataset, from)?;
            let dest = target(to);
            match (kind, to) {
                (TransferKind::Add, GraphName::NamedNode(g)) => out.push(DatasetChange::Create { graph: g.clone(), silent: true }),
                (TransferKind::Add, GraphName::DefaultGraph) => {}
                (_, GraphName::NamedNode(g)) => {
                    out.push(DatasetChange::Drop { target: GraphScope::Graph(g.clone()), silent: true });
                    out.push(DatasetChange::Create { graph: g.clone(), silent: true });
                }
                (_, GraphName::DefaultGraph) => out.push(DatasetChange::Clear { target: GraphScope::Default, silent: true }),
            }
            out.extend(triples.into_iter().map(|t| DatasetChange::Quad(dest.clone(), Change::Insert, t)));
            if *kind == TransferKind::Move {
                out.push(match from {
                    GraphName::NamedNode(g) => DatasetChange::Drop { target: GraphScope::Graph(g.clone()), silent: false },
                    GraphName::DefaultGraph => DatasetChange::Clear { target: GraphScope::Default, silent: false },
                });
            }
        }
    }
    Ok(out)
}

/// Whether the named graph `g` is in the catalog of `dataset` (empty graphs included).
fn graph_exists<S: Snapshot>(snap: &S, dataset: &dyn DatasetView, g: &triplelight_rdf::NamedNode) -> Result<bool, QueryError> {
    let storage = |e: crate::storage::StorageError| QueryError::new(QueryErrorKind::Storage, e.to_string());
    match snap.lookup(&Term::NamedNode(g.clone())).map_err(storage)? {
        Some(id) => Ok(dataset.graph(id).map_err(storage)?.is_some()),
        None => Ok(false),
    }
}

/// Every triple of the default graph or of the named graph `g`.
fn graph_triples<S: Snapshot>(snap: &S, dataset: &dyn DatasetView, g: &crate::sparql::GraphName) -> Result<Vec<triplelight_rdf::Triple>, QueryError> {
    use crate::sparql::GraphName;
    let v = |n: &str| TermPattern::Variable(Variable::new_unchecked(n));
    let bgp = GraphPattern::Bgp { patterns: vec![TriplePattern {
        subject: v("s"), predicate: NamedNodePattern::Variable(Variable::new_unchecked("p")), object: v("o") }] };
    let pattern = match g {
        GraphName::DefaultGraph => bgp,
        GraphName::NamedNode(n) => GraphPattern::Graph { name: NamedNodePattern::NamedNode(n.clone()), inner: Box::new(bgp) },
    };
    let q = Query::Select { dataset: None, pattern, base_iri: None };
    let QueryResult::Solutions { vars, rows } = catching_storage(|| run_query(snap, Some(dataset), &q, &Options::default(), udf::QueryExtensions::default()))? else { unreachable!() };
    let col = |name: &str| vars.iter().position(|x| x.as_str() == name).expect("projected variable");
    let (s, p, o) = (col("s"), col("p"), col("o"));
    Ok(rows.into_iter().filter_map(|r| match (r[s].clone(), r[p].clone(), r[o].clone()) {
        (Some(s), Some(Term::NamedNode(p)), Some(o)) => triplelight_rdf::NamedOrBlankNode::try_from(s).ok().map(|s| triplelight_rdf::Triple::new(s, p, o)),
        _ => None,
    }).collect())
}

/// The term of a VALUES / DELETE DATA constant.
fn ground_to_term(g: &GroundTerm) -> Term {
    match g {
        GroundTerm::NamedNode(n) => n.clone().into(),
        GroundTerm::Literal(l) => l.clone().into(),
        #[cfg(feature = "rdf-12")]
        GroundTerm::Triple(t) => triplelight_rdf::Triple::new(t.subject.clone(), t.predicate.clone(), ground_to_term(&t.object)).into(),
    }
}

/// `term` with every blank node `b` replaced by `f(b)`, including (SPARQL 1.2 / RDF 1.2) blank nodes inside triple
/// terms. Used to scope blank-node labels of inserted and loaded data.
pub fn map_blank_nodes(term: Term, f: &mut dyn FnMut(&BlankNode) -> BlankNode) -> Term {
    match term {
        Term::BlankNode(b) => Term::BlankNode(f(&b)),
        #[cfg(feature = "rdf-12")]
        Term::Triple(t) => {
            let t = *t;
            let subject = match t.subject { triplelight_rdf::NamedOrBlankNode::BlankNode(b) => triplelight_rdf::NamedOrBlankNode::BlankNode(f(&b)), s => s };
            triplelight_rdf::Triple::new(subject, t.predicate, map_blank_nodes(t.object, f)).into()
        }
        t => t,
    }
}

/// The predicate of an update template under a solution (`var`); `None` if unbound or not an IRI.
#[cfg(feature = "rdf-12")]
fn template_predicate(p: &NamedNodePattern, var: &dyn Fn(&Variable) -> Option<Term>) -> Option<triplelight_rdf::NamedNode> {
    match p {
        NamedNodePattern::NamedNode(x) => Some(x.clone()),
        NamedNodePattern::Variable(v) => match var(v) { Some(Term::NamedNode(x)) => Some(x), _ => None },
    }
}

/// A triple term of an INSERT template under a solution (`var`), template blank nodes mapped by `blank`; `None` if a
/// slot is unbound or ill-typed (the template quad is then skipped).
#[cfg(feature = "rdf-12")]
fn triple_template(t: &TriplePattern, var: &dyn Fn(&Variable) -> Option<Term>, blank: &dyn Fn(&BlankNode) -> BlankNode) -> Option<Term> {
    let term = |x: &TermPattern| -> Option<Term> { match x {
        TermPattern::NamedNode(n) => Some(n.clone().into()),
        TermPattern::Literal(l) => Some(l.clone().into()),
        TermPattern::BlankNode(b) => Some(blank(b).into()),
        TermPattern::Variable(v) => var(v),
        TermPattern::Triple(t) => triple_template(t, var, blank),
    } };
    let s = triplelight_rdf::NamedOrBlankNode::try_from(term(&t.subject)?).ok()?;
    let p = template_predicate(&t.predicate, var)?;
    Some(triplelight_rdf::Triple::new(s, p, term(&t.object)?).into())
}

/// A triple term of a DELETE template under a solution (`var`).
#[cfg(feature = "rdf-12")]
fn ground_triple_template(t: &crate::sparql::GroundTriplePattern, var: &dyn Fn(&Variable) -> Option<Term>) -> Option<Term> {
    use crate::sparql::GroundTermPattern as G;
    let term = |x: &G| -> Option<Term> { match x {
        G::NamedNode(n) => Some(n.clone().into()),
        G::Literal(l) => Some(l.clone().into()),
        G::Variable(v) => var(v),
        G::Triple(t) => ground_triple_template(t, var),
    } };
    let s = triplelight_rdf::NamedOrBlankNode::try_from(term(&t.subject)?).ok()?;
    let p = template_predicate(&t.predicate, var)?;
    Some(triplelight_rdf::Triple::new(s, p, term(&t.object)?).into())
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
        Expression::FunctionCall(Function::Custom(iri), _) if !expr::builtin_iri(iri.as_str()) => true,
        e => e.children().any(impure),
    }
}

/// `t` with variable `from` replaced by `to` (subject, predicate and object positions).
fn rename_var(t: &TriplePattern, from: &Variable, to: &Variable) -> TriplePattern {
    // Also inside triple-term patterns (SPARQL 1.2).
    let tp = |x: &TermPattern| match x {
        TermPattern::Variable(v) if v == from => TermPattern::Variable(to.clone()),
        #[cfg(feature = "rdf-12")]
        TermPattern::Triple(t) => TermPattern::Triple(Box::new(rename_var(t, from, to))),
        x => x.clone(),
    };
    let pp = match &t.predicate { NamedNodePattern::Variable(v) if v == from => NamedNodePattern::Variable(to.clone()), p => p.clone() };
    TriplePattern { subject: tp(&t.subject), predicate: pp, object: tp(&t.object) }
}
