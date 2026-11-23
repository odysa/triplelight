//! Per-query phase timing for the evaluation's "where the time goes" breakdown (plan amendment 1, S6, task #100).
//! Off unless `TRIPLELIGHT_TRACE_PHASES` is set; when off, each hook costs one cached flag read. Logging only: no answer
//! or plan depends on it. Counters are process-global and reset by `take`, so they are exact for single-client runs.
//! `exec` (in the server's line) includes `parse`. A count-shortcut attempt that declines (falls back to evaluation)
//! still counts its attempt as `count` time.
//! Phases are exclusive of each other: BGP time is measured only at the outermost BGP call on a thread (a FILTER
//! EXISTS inside a BGP is not counted twice), grouping and sorting are timed after their input is evaluated.
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::OnceLock;
use std::time::Instant;

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("TRIPLELIGHT_TRACE_PHASES").is_some())
}

pub(crate) static PARSE_NS: AtomicU64 = AtomicU64::new(0);
pub(crate) static BGP_NS: AtomicU64 = AtomicU64::new(0);
pub(crate) static GROUP_NS: AtomicU64 = AtomicU64::new(0);
pub(crate) static COUNT_NS: AtomicU64 = AtomicU64::new(0);
pub(crate) static SORT_NS: AtomicU64 = AtomicU64::new(0);

thread_local! { static BGP_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) }; }

/// Times `f` into `counter` when tracing is on.
pub(crate) fn timed<T>(counter: &AtomicU64, f: impl FnOnce() -> T) -> T {
    if !enabled() { return f() }
    let t = Instant::now();
    let out = f();
    counter.fetch_add(t.elapsed().as_nanos() as u64, Relaxed);
    out
}

/// Times an outermost BGP evaluation on this thread; nested ones run untimed.
pub(crate) fn bgp<T>(f: impl FnOnce() -> T) -> T {
    if !enabled() { return f() }
    let outer = BGP_DEPTH.with(|d| { let v = d.get(); d.set(v + 1); v == 0 });
    let t = outer.then(Instant::now);
    let out = f();
    BGP_DEPTH.with(|d| d.set(d.get() - 1));
    if let Some(t) = t { BGP_NS.fetch_add(t.elapsed().as_nanos() as u64, Relaxed); }
    out
}

/// Phase totals in microseconds since the last call, then reset: `parse bgp group count sort`.
pub fn take() -> [u64; 5] {
    [&PARSE_NS, &BGP_NS, &GROUP_NS, &COUNT_NS, &SORT_NS].map(|c| c.swap(0, Relaxed) / 1000)
}
