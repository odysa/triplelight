//! Cancellation and deadlines of running queries (`Options::interrupt`, #110).
use crate::QueryErrorKind;

/// Stops a running query from outside it (#110): `cancel()` from any thread, or a deadline. The executor checks it at
/// every operator and every few thousand rows inside the long loops (BGP extends, grouping, sorting, path closures),
/// and a hit ends the query with `QueryErrorKind::Cancelled` or `Timeout`, never with a partial answer. A check is a
/// relaxed atomic load plus, with a deadline, a clock read.
#[derive(Debug, Default)]
pub struct Interrupt { cancelled: std::sync::atomic::AtomicBool, deadline: Option<std::time::Instant> }

impl Interrupt {
    /// Never times out; stops only when cancelled.
    pub fn new() -> Self { Self::default() }
    /// Stops at `deadline` (or when cancelled before).
    pub fn with_deadline(deadline: std::time::Instant) -> Self { Interrupt { deadline: Some(deadline), ..Self::default() } }
    /// Stops `timeout` from now (or when cancelled before).
    pub fn with_timeout(timeout: std::time::Duration) -> Self { Self::with_deadline(std::time::Instant::now() + timeout) }
    /// Asks the query to stop at its next check; callable from any thread, any number of times.
    pub fn cancel(&self) { self.cancelled.store(true, std::sync::atomic::Ordering::Relaxed) }
    pub fn is_cancelled(&self) -> bool { self.cancelled.load(std::sync::atomic::Ordering::Relaxed) }
    /// Unwinds the query if it was cancelled or is past its deadline (caught by `catching_storage`).
    pub(crate) fn check(&self) {
        // `resume_unwind`, not `panic!`: a stop is not a bug, so the panic hook (which prints) does not run.
        if self.is_cancelled() { std::panic::resume_unwind(Box::new(Interrupted(QueryErrorKind::Cancelled))) }
        if self.deadline.is_some_and(|d| std::time::Instant::now() >= d) { std::panic::resume_unwind(Box::new(Interrupted(QueryErrorKind::Timeout))) }
    }
}

/// The panic payload of a stopped query (`Interrupt::check`).
pub(crate) struct Interrupted(pub(crate) QueryErrorKind);
