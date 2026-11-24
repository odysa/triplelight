//! Per-BGP runtime tracing and request-scoped explanations. EXPLAIN adds the observed algebra-operator tree, inclusive
//! row counts/timings and request-local phase totals. Trace hooks record observations independently of query planning.
//! Ordinary queries only check the process flag and thread-local capture slots unless tracing is on.
//! One stderr line per BGP evaluation, prefixed `plan_trace ` (greppable next to `phase_trace`):
//! `plan_trace strategy=planner patterns=3 in=1 out=42 us=310 ranges=0 order=2,0,1 | #2[?p type ProductType] access=probe
//! est=120.0 in=1 out=120 us=40 | ...` (one line). Per step: the pattern (index and compact text, IRIs cut to their local
//! name), the access path `extend` took, the planner's estimate of the subplan ending there (only when the planner chose
//! the order), input rows, rows after the extension, rows kept by pushed-down filters (`kept=`, only if fewer), and
//! microseconds. Strategies: `greedy`, `planner`, `leapfrog` (one step for the whole join), `ground` (no variables),
//! `empty` (no input rows or an absent constant), and the count shortcut (`count-leapfrog`, `count-binary-planner`,
//! `count-binary-greedy`, `count-single`, `count-empty`; no steps). A pushed-down range shows as `ranges=N` and
//! `access=range` on its step.
use crate::sparql::{GraphPattern, NamedNodePattern, TermPattern, TriplePattern};
use crate::storage::{DatasetView, Snapshot};
use crate::{Options, QueryError, QueryResult, query_dataset_with, query_dataset_with_provider, query_with};
use crate::storage::EntailmentProvider;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::OnceLock;
use std::time::Instant;

static ON: OnceLock<bool> = OnceLock::new();

pub fn enabled() -> bool { logging_enabled() || CAPTURE.with(|slot| slot.borrow().is_some()) }

fn logging_enabled() -> bool { *ON.get_or_init(|| std::env::var_os("TRIPLELIGHT_TRACE_PLAN").is_some()) }

/// Turns the trace on for this process, for tests. Effective only before the first `enabled` call (the flag is read
/// once), so a test using it must run in its own test binary; returns whether the trace is now on.
#[doc(hidden)]
pub fn force_on() -> bool { let _ = ON.set(true); logging_enabled() }

thread_local! { static CAPTURE: RefCell<Option<Capture>> = const { RefCell::new(None) }; }

struct Capture {
    lines: Vec<String>,
    remaining: usize,
    truncated: bool,
    operators: Vec<OperatorNode>,
    operator_stack: Vec<Option<usize>>,
    operator_nodes: HashMap<(Option<usize>, usize), usize>,
    operator_truncated: bool,
}

struct OperatorNode { depth: usize, label: String, calls: usize, bindings_in: usize, rows_out: usize, us: u128 }

struct CaptureScope(Option<Capture>);

impl Drop for CaptureScope {
    fn drop(&mut self) {
        CAPTURE.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}

/// Runs `f` with this thread's trace lines collected instead of printed, for tests.
#[doc(hidden)]
pub fn capture<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
    let (out, lines, _, _, _) = capture_bounded(usize::MAX, f);
    (out, lines)
}

fn capture_bounded<T>(limit: usize, f: impl FnOnce() -> T) -> (T, Vec<String>, bool, Vec<OperatorNode>, bool) {
    let _scope = CaptureScope(CAPTURE.with(|slot| slot.borrow_mut().replace(Capture {
        lines: Vec::new(),
        remaining: limit,
        truncated: false,
        operators: Vec::new(),
        operator_stack: Vec::new(),
        operator_nodes: HashMap::new(),
        operator_truncated: false,
    })));
    let out = f();
    let capture = CAPTURE.with(|slot| slot.borrow_mut().take().expect("active trace capture"));
    (out, capture.lines, capture.truncated, capture.operators, capture.operator_truncated)
}

fn emit(line: String) {
    let line = CAPTURE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(capture) = slot.as_mut() else { return Some(line) };
        if !capture.truncated && line.len() < capture.remaining {
            capture.remaining -= line.len() + 1;
            capture.lines.push(line);
        } else {
            capture.truncated = true;
        }
        None
    });
    if let Some(line) = line { eprintln!("{line}") }
}

struct Step { pattern: Option<usize>, access: &'static str, rows_in: usize, rows_out: usize, kept: usize, us: u128 }

pub(crate) struct OperatorGuard { index: Option<usize>, started: Option<Instant>, active: bool }

impl OperatorGuard {
    pub(crate) fn finish(mut self, rows_out: usize) { self.close(Some(rows_out)); }
    fn close(&mut self, rows_out: Option<usize>) {
        if !std::mem::take(&mut self.active) { return }
        CAPTURE.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(capture) = slot.as_mut() else { return };
            let popped = capture.operator_stack.pop().flatten();
            let Some(index) = self.index.take() else { return };
            if popped != Some(index) { capture.operator_stack.clear(); }
            let node = &mut capture.operators[index];
            node.us += self.started.take().expect("active operator timer").elapsed().as_micros();
            if let Some(rows_out) = rows_out { node.rows_out += rows_out; }
        });
    }
}

impl Drop for OperatorGuard { fn drop(&mut self) { self.close(None); } }

/// Begins one observed algebra operator. The guard is inert outside an EXPLAIN capture.
pub(crate) fn operator(pattern: &GraphPattern, rows_in: usize) -> OperatorGuard {
    const OPERATOR_LIMIT: usize = 1024;
    let (active, index) = CAPTURE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(capture) = slot.as_mut() else { return (false, None) };
        if capture.operator_stack.last() == Some(&None) {
            capture.operator_stack.push(None);
            return (true, None)
        }
        let parent = capture.operator_stack.last().copied().flatten();
        let identity = pattern as *const GraphPattern as usize;
        if let Some(&index) = capture.operator_nodes.get(&(parent, identity)) {
            let node = &mut capture.operators[index];
            node.calls += 1;
            node.bindings_in += rows_in;
            capture.operator_stack.push(Some(index));
            return (true, Some(index))
        }
        if capture.operators.len() == OPERATOR_LIMIT {
            capture.operator_truncated = true;
            capture.operator_stack.push(None);
            return (true, None)
        }
        let index = capture.operators.len();
        capture.operators.push(OperatorNode {
            depth: capture.operator_stack.len(), label: operator_label(pattern), calls: 1,
            bindings_in: rows_in, rows_out: 0, us: 0,
        });
        capture.operator_nodes.insert((parent, identity), index);
        capture.operator_stack.push(Some(index));
        (true, Some(index))
    });
    OperatorGuard { index, started: index.map(|_| Instant::now()), active }
}

fn operator_label(pattern: &GraphPattern) -> String {
    match pattern {
        GraphPattern::Bgp { patterns } => format!("BGP patterns={}", patterns.len()),
        GraphPattern::Join { .. } => "Join".into(),
        GraphPattern::LeftJoin { expression, .. } => format!("LeftJoin condition={}", expression.is_some()),
        GraphPattern::Filter { .. } => "Filter".into(),
        GraphPattern::Union { .. } => "Union".into(),
        GraphPattern::Extend { variable, .. } => format!("Extend ?{}", variable.as_str()),
        GraphPattern::Minus { .. } => "Minus".into(),
        GraphPattern::Values { variables, bindings } => format!("Values variables={} rows={}", variables.len(), bindings.len()),
        GraphPattern::OrderBy { expression, .. } => format!("OrderBy keys={}", expression.len()),
        GraphPattern::Project { variables, .. } => format!("Project variables={}", variables.len()),
        GraphPattern::Distinct { .. } => "Distinct".into(),
        GraphPattern::Reduced { .. } => "Reduced".into(),
        GraphPattern::Slice { start, length, .. } => format!("Slice offset={start} limit={}", length.map_or_else(|| "none".into(), |n| n.to_string())),
        GraphPattern::Group { variables, aggregates, .. } => format!("Group keys={} aggregates={}", variables.len(), aggregates.len()),
        GraphPattern::Graph { .. } => "Graph".into(),
        GraphPattern::Path { .. } => "Path".into(),
        GraphPattern::Service { .. } => "Service".into(),
    }
}

/// One BGP evaluation's trace; built only when `enabled()`.
pub(crate) struct Trace {
    start: Instant,
    patterns: Vec<String>,
    rows_in: usize,
    notes: String,
    order: Option<Vec<usize>>,
    /// Planner estimate by pattern index.
    est: Option<Vec<f64>>,
    steps: Vec<Step>,
}

impl Trace {
    pub(crate) fn new(patterns: &[TriplePattern], rows_in: usize) -> Self {
        Trace { start: Instant::now(), patterns: patterns.iter().map(pattern_text).collect(), rows_in, notes: String::new(), order: None, est: None, steps: Vec::new() }
    }

    /// Appends ` key=value` to the line header.
    pub(crate) fn note(&mut self, key: &str, value: impl std::fmt::Display) { let _ = write!(self.notes, " {key}={value}"); }

    pub(crate) fn planned(&mut self, order: &[usize], est: Option<Vec<f64>>) { self.order = Some(order.to_vec()); self.est = est; }

    /// Records one join step that started at `t`. Row counts are rows, not IDs.
    pub(crate) fn step(&mut self, pattern: Option<usize>, access: &'static str, rows_in: usize, rows_out: usize, kept: usize, t: Instant) {
        self.steps.push(Step { pattern, access, rows_in, rows_out, kept, us: t.elapsed().as_micros() });
    }

    /// Emits the line.
    pub(crate) fn finish(self, strategy: &str, rows_out: usize) {
        let mut s = format!("plan_trace strategy={strategy} patterns={} in={} out={rows_out} us={}{}", self.patterns.len(), self.rows_in, self.start.elapsed().as_micros(), self.notes);
        let order: Vec<usize> = match &self.order { Some(o) => o.clone(), None => self.steps.iter().filter_map(|st| st.pattern).collect() };
        if !order.is_empty() { let _ = write!(s, " order={}", order.iter().map(usize::to_string).collect::<Vec<_>>().join(",")); }
        for st in &self.steps {
            match st.pattern {
                Some(i) => { let _ = write!(s, " | #{i}[{}]", self.patterns[i]); }
                None => { let _ = write!(s, " | [{}]", self.patterns.join(" . ")); }
            }
            let _ = write!(s, " access={}", st.access);
            if let (Some(est), Some(i)) = (&self.est, st.pattern) { let _ = write!(s, " est={:.1}", est[i]); }
            let _ = write!(s, " in={} out={}", st.rows_in, st.rows_out);
            if st.kept != st.rows_out { let _ = write!(s, " kept={}", st.kept); }
            let _ = write!(s, " us={}", st.us);
        }
        emit(s);
    }
}

/// `?name`, the local name of an IRI (after its last '#' or '/'), `"lexical"`, `_:id`.
fn term_text(t: &TermPattern) -> String {
    match t {
        TermPattern::Variable(v) => format!("?{}", v.as_str()),
        TermPattern::NamedNode(n) => local(n.as_str()).to_string(),
        TermPattern::BlankNode(b) => format!("_:{}", b.as_str()),
        TermPattern::Literal(l) => format!("{:?}", l.value()),
        #[cfg(feature = "rdf-12")]
        TermPattern::Triple(t) => format!("<<( {} )>>", pattern_text(t)),
    }
}

fn local(iri: &str) -> &str { iri.rsplit(['#', '/']).find(|s| !s.is_empty()).unwrap_or(iri) }

fn pattern_text(t: &TriplePattern) -> String {
    let p = match &t.predicate { NamedNodePattern::Variable(v) => format!("?{}", v.as_str()), NamedNodePattern::NamedNode(n) => local(n.as_str()).to_string() };
    format!("{} {p} {}", term_text(&t.subject), term_text(&t.object))
}

/// `?a,?b` for a leapfrog variable order over row slots, naming each slot from `vars`.
pub(crate) fn var_names(order: &[usize], vars: &std::collections::HashMap<String, usize>) -> String {
    order.iter().map(|&i| match vars.iter().find(|&(_, &j)| j == i) {
        Some((n, _)) if n.starts_with("_:") => n.clone(),
        Some((n, _)) => format!("?{n}"),
        None => format!("#{i}"),
    }).collect::<Vec<_>>().join(",")
}

const TRACE_LIMIT: usize = 256 * 1024;

/// Executes against a single graph and returns the observed BGP plan as text.
pub fn explain<S: Snapshot>(snapshot: &S, text: &str, options: &Options<'_>) -> Result<String, QueryError> {
    report(|| query_with(snapshot, text, &Options { raw_ids: true, ..*options }))
}

/// Executes against a dataset and returns the observed BGP plan as text.
pub fn explain_dataset<S: Snapshot>(snapshot: &S, dataset: &dyn DatasetView, text: &str, options: &Options<'_>) -> Result<String, QueryError> {
    report(|| query_dataset_with(snapshot, dataset, text, &Options { raw_ids: true, ..*options }))
}

/// Executes a dataset query using the selected entailment provider and formats its observed plan.
pub fn explain_dataset_with_provider<S: Snapshot>(
    snapshot: &S,
    dataset: &dyn DatasetView,
    text: &str,
    options: &Options<'_>,
    provider: &dyn EntailmentProvider,
) -> Result<String, QueryError> {
    report(|| query_dataset_with_provider(snapshot, dataset, text, &Options { raw_ids: true, ..*options }, provider))
}

#[doc(hidden)]
pub fn explain_dataset_with_extensions<S: Snapshot>(
    snapshot: &S,
    dataset: &dyn DatasetView,
    text: &str,
    options: &Options<'_>,
    extensions: crate::udf::QueryExtensions<'_>,
) -> Result<String, QueryError> {
    report(|| crate::query_with_extensions(snapshot, Some(dataset), text, &Options { raw_ids: true, ..*options }, extensions))
}

fn report(run: impl FnOnce() -> Result<QueryResult, QueryError>) -> Result<String, QueryError> {
    let ((result, lines, truncated, operators, operator_truncated), phases) = crate::phase::capture(|| capture_bounded(TRACE_LIMIT, run));
    drop(result?);
    let mut report = String::from("Observed operator tree\n");
    if operators.is_empty() { report.push_str("(empty)\n"); }
    let tree_start = report.len();
    let mut render_truncated = false;
    for node in operators {
        let line = format!("{}- {} calls={} bindings_in={} rows_out={} us={}\n", "  ".repeat(node.depth), node.label, node.calls, node.bindings_in, node.rows_out, node.us);
        if report.len() + line.len() - tree_start > TRACE_LIMIT { render_truncated = true; break }
        report.push_str(&line);
    }
    if operator_truncated || render_truncated { report.push_str("Operator tree truncated.\n"); }
    let [parse, bgp, group, count, sort, filter, extend, join] = phases;
    let _ = writeln!(report, "Phase timings (microseconds; nested work may overlap)\nparse_us={parse} bgp_us={bgp} group_us={group} count_us={count} sort_us={sort} filter_us={filter} extend_us={extend} join_us={join}");
    report.push_str("Observed basic graph pattern plan\n");
    if lines.is_empty() && !truncated { report.push_str("No basic graph pattern was traced.\n"); }
    for line in lines {
        report.push_str(&line);
        report.push('\n');
    }
    if truncated { report.push_str("Plan trace truncated.\n"); }
    Ok(report)
}
