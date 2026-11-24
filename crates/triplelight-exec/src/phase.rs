//! Per-query phase timing for the evaluation's "where the time goes" breakdown (plan amendment 1, S6, task #100).
//! Off unless `TRIPLELIGHT_TRACE_PHASES` is set or an EXPLAIN request installs a thread-local capture. Logging only: no
//! answer or plan depends on it. Process-global counters are exact for single-client trace runs; EXPLAIN captures are
//! request-scoped and remain exact when the server handles concurrent requests.
//! `exec` (in the server's line) includes `parse`. A count-shortcut attempt that declines (falls back to evaluation)
//! still counts its attempt as `count` time.
//! Phases are exclusive of each other: BGP time is measured only at the outermost BGP call on a thread (a FILTER
//! EXISTS inside a BGP is not counted twice); grouping, sorting, filtering and extending are timed after their input.
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::OnceLock;
use std::time::Instant;

const PARSE: usize = 0;
const BGP: usize = 1;
const GROUP: usize = 2;
const COUNT: usize = 3;
const SORT: usize = 4;
const FILTER: usize = 5;
const EXTEND: usize = 6;
const JOIN: usize = 7;
const PHASES: usize = 8;

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("TRIPLELIGHT_TRACE_PHASES").is_some())
}

pub(crate) static PARSE_NS: AtomicU64 = AtomicU64::new(0);
pub(crate) static BGP_NS: AtomicU64 = AtomicU64::new(0);
pub(crate) static GROUP_NS: AtomicU64 = AtomicU64::new(0);
pub(crate) static COUNT_NS: AtomicU64 = AtomicU64::new(0);
pub(crate) static SORT_NS: AtomicU64 = AtomicU64::new(0);
pub(crate) static FILTER_NS: AtomicU64 = AtomicU64::new(0);
pub(crate) static EXTEND_NS: AtomicU64 = AtomicU64::new(0);
pub(crate) static JOIN_NS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static BGP_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static CAPTURE: std::cell::RefCell<Option<[u128; PHASES]>> = const { std::cell::RefCell::new(None) };
}

fn active() -> bool { enabled() || CAPTURE.with(|capture| capture.borrow().is_some()) }

fn record(slot: usize, counter: &AtomicU64, ns: u128) {
    if enabled() { counter.fetch_add(ns as u64, Relaxed); }
    CAPTURE.with(|capture| {
        if let Some(totals) = capture.borrow_mut().as_mut() { totals[slot] += ns; }
    });
}

/// Times `f` into `counter` when tracing is on.
fn timed<T>(slot: usize, counter: &AtomicU64, f: impl FnOnce() -> T) -> T {
    if !active() { return f() }
    let t = Instant::now();
    let out = f();
    record(slot, counter, t.elapsed().as_nanos());
    out
}

pub(crate) fn parse<T>(f: impl FnOnce() -> T) -> T { timed(PARSE, &PARSE_NS, f) }
pub(crate) fn group<T>(f: impl FnOnce() -> T) -> T { timed(GROUP, &GROUP_NS, f) }
pub(crate) fn count<T>(f: impl FnOnce() -> T) -> T { timed(COUNT, &COUNT_NS, f) }
pub(crate) fn sort<T>(f: impl FnOnce() -> T) -> T { timed(SORT, &SORT_NS, f) }
pub(crate) fn filter<T>(f: impl FnOnce() -> T) -> T { timed(FILTER, &FILTER_NS, f) }
pub(crate) fn extend<T>(f: impl FnOnce() -> T) -> T { timed(EXTEND, &EXTEND_NS, f) }
pub(crate) fn join<T>(f: impl FnOnce() -> T) -> T { timed(JOIN, &JOIN_NS, f) }

/// Times an outermost BGP evaluation on this thread; nested ones run untimed.
pub(crate) fn bgp<T>(f: impl FnOnce() -> T) -> T {
    if !active() { return f() }
    let outer = BGP_DEPTH.with(|d| { let v = d.get(); d.set(v + 1); v == 0 });
    let t = outer.then(Instant::now);
    let out = f();
    BGP_DEPTH.with(|d| d.set(d.get() - 1));
    if let Some(t) = t { record(BGP, &BGP_NS, t.elapsed().as_nanos()); }
    out
}

static RANGES: std::sync::Mutex<Vec<(bool, f64)>> = std::sync::Mutex::new(Vec::new());

/// Records one range-pushdown decision (`range.rs`) when tracing is on: whether the range was pushed and the
/// fraction of the predicate's triples it covers (a lower bound once it exceeds the gate, where counting stops).
pub(crate) fn range_decision(pushed: bool, frac: f64) {
    if enabled() && let Ok(mut r) = RANGES.lock() { r.push((pushed, frac)) }
}

/// The range-pushdown decisions since the last call, then reset: `on:0.031,off:0.150` in decision order, `-` if none.
pub fn take_ranges() -> String {
    let r = RANGES.lock().map(|mut r| std::mem::take(&mut *r)).unwrap_or_default();
    if r.is_empty() { return "-".into() }
    r.iter().map(|(on, f)| format!("{}:{f:.3}", if *on { "on" } else { "off" })).collect::<Vec<_>>().join(",")
}

/// Phase totals in microseconds since the last call, then reset:
/// `parse bgp group count sort filter extend join`.
pub fn take() -> [u64; 8] {
    [&PARSE_NS, &BGP_NS, &GROUP_NS, &COUNT_NS, &SORT_NS, &FILTER_NS, &EXTEND_NS, &JOIN_NS]
        .map(|c| c.swap(0, Relaxed) / 1000)
}

/// Request-local phase totals in microseconds. Unlike the process counters, these are safe under concurrent EXPLAIN.
pub(crate) fn capture<T>(f: impl FnOnce() -> T) -> (T, [u64; PHASES]) {
    struct Scope(Option<[u128; PHASES]>);
    impl Drop for Scope {
        fn drop(&mut self) { CAPTURE.with(|capture| *capture.borrow_mut() = self.0.take()); }
    }
    let previous = CAPTURE.with(|capture| capture.borrow_mut().replace([0; PHASES]));
    let mut scope = Scope(previous);
    let out = f();
    let totals = CAPTURE.with(|capture| capture.borrow_mut().take().expect("active phase capture"));
    CAPTURE.with(|capture| *capture.borrow_mut() = scope.0.take());
    (out, totals.map(|ns| (ns / 1000) as u64))
}
