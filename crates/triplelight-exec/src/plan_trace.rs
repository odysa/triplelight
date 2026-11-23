//! Per-BGP plan trace. Off unless `TRIPLELIGHT_TRACE_PLAN` is set; when off, each hook costs one cached flag read
//! (no clock, no allocation). Logging only: no answer or plan depends on it.
//! One stderr line per BGP evaluation, prefixed `plan_trace ` (greppable next to `phase_trace`):
//! `plan_trace strategy=planner patterns=3 in=1 out=42 us=310 ranges=0 order=2,0,1 | #2[?p type ProductType] access=probe
//! est=120.0 in=1 out=120 us=40 | ...` (one line). Per step: the pattern (index and compact text, IRIs cut to their local
//! name), the access path `extend` took, the planner's estimate of the subplan ending there (only when the planner chose
//! the order), input rows, rows after the extension, rows kept by pushed-down filters (`kept=`, only if fewer), and
//! microseconds. Strategies: `greedy`, `planner`, `leapfrog` (one step for the whole join), `ground` (no variables),
//! `empty` (no input rows or an absent constant), and the count shortcut (`count-leapfrog`, `count-binary-planner`,
//! `count-binary-greedy`, `count-single`, `count-empty`; no steps). A pushed-down range shows as `ranges=N` and
//! `access=range` on its step.
use crate::sparql::{NamedNodePattern, TermPattern, TriplePattern};
use std::cell::RefCell;
use std::fmt::Write as _;
use std::sync::OnceLock;
use std::time::Instant;

static ON: OnceLock<bool> = OnceLock::new();

pub fn enabled() -> bool { *ON.get_or_init(|| std::env::var_os("TRIPLELIGHT_TRACE_PLAN").is_some()) }

/// Turns the trace on for this process, for tests. Effective only before the first `enabled` call (the flag is read
/// once), so a test using it must run in its own test binary; returns whether the trace is now on.
#[doc(hidden)]
pub fn force_on() -> bool { let _ = ON.set(true); enabled() }

thread_local! { static CAPTURE: RefCell<Option<Vec<String>>> = const { RefCell::new(None) }; }

/// Runs `f` with this thread's trace lines collected instead of printed, for tests.
#[doc(hidden)]
pub fn capture<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
    let saved = CAPTURE.with(|c| c.borrow_mut().replace(Vec::new()));
    let out = f();
    let lines = CAPTURE.with(|c| std::mem::replace(&mut *c.borrow_mut(), saved)).unwrap_or_default();
    (out, lines)
}

fn emit(line: String) {
    let line = CAPTURE.with(|c| match c.borrow_mut().as_mut() { Some(v) => { v.push(line); None } None => Some(line) });
    if let Some(l) = line { eprintln!("{l}") }
}

struct Step { pattern: Option<usize>, access: &'static str, rows_in: usize, rows_out: usize, kept: usize, us: u128 }

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
