//! Per-query limits (#110): cancellation and deadlines (`Options::interrupt`) and a memory budget
//! (`Options::memory_budget`). Hitting either ends the query with its own error kind, never a partial answer.
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

/// A byte budget for one query's working memory (#110). Operators charge it for the structures they build (BGP
/// intermediate rows, GROUP BY tables, sort keys, join tables, path pair lists) and release on drop; a charge that
/// would exceed the limit ends the query with `QueryErrorKind::ResourceLimit` instead of running the process out of
/// memory. What is bounded is each operator's working set while it runs together with what other operators hold, not
/// every allocation: estimates are per row (`Ctx::row_bytes`), not exact heap sizes. A spilling operator (#111) can ask
/// `available()` to size its runs and write them under `scratch_dir()`.
#[derive(Debug)]
pub struct MemoryBudget { limit: u64, used: std::sync::atomic::AtomicU64, scratch_dir: Option<std::path::PathBuf> }

/// A charge of `requested` bytes did not fit: `available` were left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverBudget { pub requested: u64, pub available: u64 }

impl MemoryBudget {
    pub fn new(limit_bytes: u64) -> Self { MemoryBudget { limit: limit_bytes, used: Default::default(), scratch_dir: None } }
    /// Where a spilling operator may write its runs (none: it cannot spill).
    pub fn with_scratch_dir(mut self, dir: impl Into<std::path::PathBuf>) -> Self { self.scratch_dir = Some(dir.into()); self }
    pub fn limit(&self) -> u64 { self.limit }
    pub fn used(&self) -> u64 { self.used.load(std::sync::atomic::Ordering::Relaxed) }
    pub fn available(&self) -> u64 { self.limit.saturating_sub(self.used()) }
    pub fn scratch_dir(&self) -> Option<&std::path::Path> { self.scratch_dir.as_deref() }
    /// Reserves `bytes`, released when the reservation drops. Safe from any thread.
    pub fn try_charge(&self, bytes: u64) -> Result<Reservation<'_>, OverBudget> {
        self.take(bytes)?;
        Ok(Reservation { budget: self, bytes })
    }
    fn take(&self, bytes: u64) -> Result<(), OverBudget> {
        use std::sync::atomic::Ordering::Relaxed;
        let mut cur = self.used.load(Relaxed);
        loop {
            let next = cur.saturating_add(bytes);
            if next > self.limit { return Err(OverBudget { requested: bytes, available: self.limit.saturating_sub(cur) }) }
            match self.used.compare_exchange_weak(cur, next, Relaxed, Relaxed) { Ok(_) => return Ok(()), Err(c) => cur = c }
        }
    }
    fn give_back(&self, bytes: u64) { self.used.fetch_sub(bytes, std::sync::atomic::Ordering::Relaxed); }
}

/// Bytes held against a `MemoryBudget`; released on drop (also when a stopped query unwinds).
#[derive(Debug)]
pub struct Reservation<'a> { budget: &'a MemoryBudget, bytes: u64 }

impl Reservation<'_> {
    pub fn bytes(&self) -> u64 { self.bytes }
    /// Grows the reservation to `total` bytes (a table that doubled); shrinking is free and never fails.
    pub fn try_resize(&mut self, total: u64) -> Result<(), OverBudget> {
        if total > self.bytes { self.budget.take(total - self.bytes)?; } else { self.budget.give_back(self.bytes - total); }
        self.bytes = total;
        Ok(())
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) { self.budget.give_back(self.bytes) }
}

/// The panic payload of a query whose charge did not fit (`Ctx::charge`), turned into `ResourceLimit`.
pub(crate) struct OverBudgetStop(pub(crate) OverBudget);

/// Unwinds the query with `ResourceLimit` (no panic hook output, as for `Interrupt`).
pub(crate) fn over_budget(e: OverBudget) -> ! { std::panic::resume_unwind(Box::new(OverBudgetStop(e))) }

/// The limits of one query, for APIs that take them together (`Database::query_with_limits`).
#[derive(Debug, Clone, Copy, Default)]
pub struct QueryLimits<'a> { pub interrupt: Option<&'a Interrupt>, pub memory: Option<&'a MemoryBudget> }
