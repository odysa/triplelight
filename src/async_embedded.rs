//! Async embedded API (feature `async`, task #41), independent of any async runtime.
//!
//! TripleLight's query execution and storage are synchronous. Every call here runs that synchronous work on a worker
//! thread and hands the result back through an [AsyncBand](https://github.com/apache/asyncband) oneshot, so the
//! caller's executor thread never blocks and any executor (tokio, async-std, smol, a hand-written poller) can await
//! it. A semaphore bounds how many calls of one database (and its clones) run at once.
//!
//! **Cancellation.** Dropping a future while it still waits for a permit cancels it: its work never starts. Dropping
//! a query's future after its work has started cancels the query too (#110): the worker stops at its next check and
//! its result is discarded. Other calls run to the end once started; in particular an update may still commit.
//! [`QueryOptions`] adds a timeout and a working-memory budget to a query.
//!
//! This is not native asynchronous disk I/O, and it is not available for WebAssembly (the WASM binding is
//! synchronous).
use std::path::{Path, PathBuf};
use std::sync::Arc;

use asyncband::{oneshot, semaphore::Semaphore};

use crate::embedded::{self, DatabaseError, DatabaseOptions, StatsOrigin};
use crate::exec::{self, QueryResult};

/// The default bound on concurrent calls per database: the number of cores, at most 8.
fn default_concurrency() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get()).min(8)
}

/// Runs `job` on a worker thread once a permit of `limit` is free; the permit is held until `job` returns.
async fn run<T: Send + 'static>(limit: &Arc<Semaphore>, job: impl FnOnce() -> Result<T, DatabaseError> + Send + 'static) -> Result<T, DatabaseError> {
    let permit = Arc::clone(limit).acquire_owned(1).await;
    let (sender, receiver) = oneshot::channel();
    std::thread::Builder::new()
        .name("triplelight-async".into())
        .spawn(move || {
            let _permit = permit;
            let _ = sender.send(job());
        })
        .map_err(DatabaseError::Storage)?;
    receiver.await.map_err(|_| DatabaseError::Storage(std::io::Error::other("database worker terminated")))?
}

/// Options of one query: its entailment regime, a timeout and a working-memory budget. A query that runs out of
/// either ends with `DatabaseError::Interrupted { timed_out: true }` or `DatabaseError::ResourceLimit`, never with a
/// partial answer. The timeout counts from the first poll, so time spent waiting for a worker counts too.
#[derive(Debug, Clone, Copy, Default)]
pub struct QueryOptions {
    pub entailment: exec::Entailment,
    pub timeout: Option<std::time::Duration>,
    /// Bytes of working memory the query may hold at once (see [`exec::MemoryBudget`]).
    pub memory_bytes: Option<u64>,
}

/// Cancels its query when dropped: a query future dropped before it finishes takes it down with it.
struct CancelOnDrop(Arc<exec::Interrupt>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) { self.0.cancel() }
}

/// `run` for a query: `job` gets the limits `options` asks for, and dropping the future cancels the query.
async fn run_query<T: Send + 'static>(
    limit: &Arc<Semaphore>,
    options: QueryOptions,
    job: impl FnOnce(exec::Entailment, exec::QueryLimits<'_>) -> Result<T, DatabaseError> + Send + 'static,
) -> Result<T, DatabaseError> {
    let interrupt = Arc::new(options.timeout.map_or_else(exec::Interrupt::new, exec::Interrupt::with_timeout));
    let memory = options.memory_bytes.map(exec::MemoryBudget::new);
    let _cancel = CancelOnDrop(Arc::clone(&interrupt));
    run(limit, move || job(options.entailment, exec::QueryLimits { interrupt: Some(&interrupt), memory: memory.as_ref() })).await
}

/// An async handle to an embedded database. Clones share the database and its concurrency limit.
#[derive(Clone)]
pub struct Database {
    db: embedded::Database,
    limit: Arc<Semaphore>,
}

impl Database {
    /// Opens an existing database, or creates an empty one at `dir`.
    pub async fn open(dir: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        Self::open_with(dir, DatabaseOptions::default()).await
    }

    /// Opens a database with explicit options (see [`embedded::Database::open_with`]).
    pub async fn open_with(dir: impl AsRef<Path>, options: DatabaseOptions) -> Result<Self, DatabaseError> {
        let dir: PathBuf = dir.as_ref().to_owned();
        let limit = Arc::new(Semaphore::new(default_concurrency()));
        let db = run(&limit, move || embedded::Database::open_with(dir, options)).await?;
        Ok(Self { db, limit })
    }

    /// Creates an isolated, non-durable in-memory database.
    pub fn in_memory() -> Self {
        Self::from_sync(embedded::Database::in_memory())
    }

    /// Wraps an already open database (for example one opened synchronously at startup).
    pub fn from_sync(db: embedded::Database) -> Self {
        Self { db, limit: Arc::new(Semaphore::new(default_concurrency())) }
    }

    /// The same database with at most `n` calls running at once (`n` is at least 1). Clones made afterwards share it.
    pub fn with_concurrency(self, n: usize) -> Self {
        Self { db: self.db, limit: Arc::new(Semaphore::new(n.max(1))) }
    }

    /// The synchronous handle, for calls this API does not wrap.
    pub fn as_sync(&self) -> &embedded::Database { &self.db }

    /// A stable read view, unaffected by later updates.
    pub async fn read(&self) -> Result<ReadSnapshot, DatabaseError> {
        let db = self.db.clone();
        let snapshot = run(&self.limit, move || Ok(Arc::new(db.read()))).await?;
        Ok(ReadSnapshot { db: self.clone(), snapshot })
    }

    /// Executes a SPARQL query and returns owned results.
    pub async fn query(&self, sparql: &str) -> Result<QueryResult, DatabaseError> {
        self.query_with(sparql, QueryOptions::default()).await
    }

    /// `query` with a timeout, a memory budget or an entailment regime.
    pub async fn query_with(&self, sparql: &str, options: QueryOptions) -> Result<QueryResult, DatabaseError> {
        let (db, sparql) = (self.db.clone(), sparql.to_owned());
        run_query(&self.limit, options, move |entailment, limits| db.query_with_limits(&sparql, entailment, limits)).await
    }

    /// Executes a SELECT or ASK and serializes it as SPARQL Results JSON.
    pub async fn query_json(&self, sparql: &str) -> Result<String, DatabaseError> {
        self.query_json_with(sparql, QueryOptions::default()).await
    }

    /// `query_json` with a timeout, a memory budget or an entailment regime.
    pub async fn query_json_with(&self, sparql: &str, options: QueryOptions) -> Result<String, DatabaseError> {
        let (db, sparql) = (self.db.clone(), sparql.to_owned());
        run_query(&self.limit, options, move |entailment, limits| db.query_json_with_limits(&sparql, entailment, limits)).await
    }

    /// Applies a SPARQL Update atomically (durably for a persistent database). Dropping the future after the update
    /// has started does not roll it back.
    pub async fn update(&self, sparql: &str) -> Result<usize, DatabaseError> {
        let (db, sparql) = (self.db.clone(), sparql.to_owned());
        run(&self.limit, move || db.update(&sparql)).await
    }

    /// Atomically loads an N-Triples, Turtle, N-Quads or TriG file (by extension).
    pub async fn load(&self, path: impl AsRef<Path>) -> Result<usize, DatabaseError> {
        let (db, path) = (self.db.clone(), path.as_ref().to_owned());
        run(&self.limit, move || db.load(path)).await
    }

    /// Writes the current state into a new base; existing snapshots keep theirs.
    pub async fn compact(&self) -> Result<(), DatabaseError> {
        let db = self.db.clone();
        run(&self.limit, move || db.compact()).await
    }

    /// Whether planner statistics for the current base are ready (no I/O; returns at once).
    pub fn stats_ready(&self) -> bool { self.db.stats_ready() }

    /// Where the current base's planner statistics came from (no I/O; returns at once).
    pub fn stats_origin(&self) -> Option<StatsOrigin> { self.db.stats_origin() }
}

/// An immutable view of the database at the moment [`Database::read`] was called. It keeps its database open.
#[derive(Clone)]
pub struct ReadSnapshot {
    db: Database,
    snapshot: Arc<embedded::ReadSnapshot>,
}

impl ReadSnapshot {
    /// Executes a query against this snapshot.
    pub async fn query(&self, sparql: &str) -> Result<QueryResult, DatabaseError> {
        self.query_with(sparql, QueryOptions::default()).await
    }

    /// `query` on this snapshot with a timeout, a memory budget or an entailment regime.
    pub async fn query_with(&self, sparql: &str, options: QueryOptions) -> Result<QueryResult, DatabaseError> {
        let (snapshot, sparql) = (Arc::clone(&self.snapshot), sparql.to_owned());
        run_query(&self.db.limit, options, move |entailment, limits| snapshot.query_with_limits(&sparql, entailment, limits)).await
    }

    /// Serializes a SELECT or ASK on this snapshot as SPARQL Results JSON.
    pub async fn query_json(&self, sparql: &str) -> Result<String, DatabaseError> {
        self.query_json_with(sparql, QueryOptions::default()).await
    }

    /// `query_json` on this snapshot with a timeout, a memory budget or an entailment regime.
    pub async fn query_json_with(&self, sparql: &str, options: QueryOptions) -> Result<String, DatabaseError> {
        let (snapshot, sparql) = (Arc::clone(&self.snapshot), sparql.to_owned());
        run_query(&self.db.limit, options, move |entailment, limits| snapshot.query_json_with_limits(&sparql, entailment, limits)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::task::{Context, Poll, Wake, Waker};
    use std::time::Duration;

    /// A minimal executor: polls one future on this thread, parking between wake-ups. No async runtime involved.
    struct ThreadWake(std::thread::Thread);
    impl Wake for ThreadWake { fn wake(self: Arc<Self>) { self.0.unpark() } }
    fn block_on<F: Future>(future: F) -> F::Output {
        let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            if let Poll::Ready(v) = future.as_mut().poll(&mut cx) { return v }
            std::thread::park_timeout(Duration::from_secs(5));
        }
    }
    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("triplelight-async-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }
    fn ask(json: &str) -> bool { json.contains("true") }

    #[test]
    fn persistent_round_trip_snapshot_isolation_and_reopen() {
        let dir = temp_dir("persistent");
        block_on(async {
            let db = Database::open(&dir).await.unwrap();
            assert_eq!(db.update("INSERT DATA { <urn:s> <urn:p> <urn:o> GRAPH <urn:g> { <urn:s> <urn:p> <urn:o2> } }").await.unwrap(), 2);
            assert!(ask(&db.query_json("ASK { <urn:s> <urn:p> <urn:o> }").await.unwrap()));
            let snapshot = db.read().await.unwrap();
            db.update("DELETE DATA { <urn:s> <urn:p> <urn:o> }").await.unwrap();
            assert!(ask(&snapshot.query_json("ASK { <urn:s> <urn:p> <urn:o> }").await.unwrap()), "the snapshot keeps its view");
            assert!(!ask(&db.query_json("ASK { <urn:s> <urn:p> <urn:o> }").await.unwrap()));
            db.compact().await.unwrap();
            assert!(ask(&db.query_json("ASK { GRAPH <urn:g> { <urn:s> <urn:p> <urn:o2> } }").await.unwrap()));
        });
        // Reopen after every handle is dropped: the update is durable.
        block_on(async {
            let db = Database::open(&dir).await.unwrap();
            assert!(ask(&db.query_json("ASK { GRAPH <urn:g> { <urn:s> <urn:p> <urn:o2> } }").await.unwrap()));
            assert!(matches!(db.query("SELECT * { ?s ?p ?o }").await.unwrap(), QueryResult::Solutions { .. }));
        });
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn in_memory_and_load() {
        let dir = temp_dir("load");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("data.nq");
        std::fs::write(&file, "<urn:a> <urn:p> \"1\" .\n<urn:a> <urn:p> \"2\" <urn:g> .\n").unwrap();
        block_on(async {
            let db = Database::in_memory();
            assert_eq!(db.load(&file).await.unwrap(), 2);
            assert!(ask(&db.query_json("ASK { GRAPH <urn:g> { <urn:a> <urn:p> \"2\" } }").await.unwrap()));
            assert!(db.update("NOT SPARQL").await.is_err(), "errors come back through the future");
        });
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The caller's thread is never blocked: while the work runs, polling returns `Pending` at once.
    #[test]
    fn polling_never_blocks_the_caller() {
        let limit = Arc::new(Semaphore::new(1));
        let (started_tx, started_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel::<()>();
        let mut future = Box::pin(run(&limit, move || { started_tx.send(()).unwrap(); finish_rx.recv().unwrap(); Ok(7) }));
        let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        assert!(future.as_mut().poll(&mut cx).is_pending());
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let t = std::time::Instant::now();
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert!(t.elapsed() < Duration::from_millis(100), "a poll returned only after {:?}", t.elapsed());
        finish_tx.send(()).unwrap();
        assert_eq!(block_on(future).unwrap(), 7);
    }

    /// Dropping a future whose work has started does not stop it (an update still commits); dropping one that is
    /// still waiting for a permit cancels it before it starts.
    #[test]
    fn cancellation_semantics() {
        let limit = Arc::new(Semaphore::new(1));
        let ran = Arc::new(AtomicUsize::new(0));
        let (started_tx, started_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel::<()>();
        let (done_tx, done_rx) = mpsc::channel();
        let r = Arc::clone(&ran);
        let mut first = Box::pin(run(&limit, move || {
            started_tx.send(()).unwrap();
            finish_rx.recv().unwrap();
            r.fetch_add(1, Ordering::SeqCst);
            done_tx.send(()).unwrap();
            Ok(())
        }));
        let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        assert!(first.as_mut().poll(&mut cx).is_pending());
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // The only permit is taken, so this one waits; dropping it must mean its work never runs.
        let r = Arc::clone(&ran);
        let mut queued = Box::pin(run(&limit, move || { r.fetch_add(100, Ordering::SeqCst); Ok(()) }));
        assert!(queued.as_mut().poll(&mut cx).is_pending());
        drop(queued);
        drop(first); // started: keeps running
        finish_tx.send(()).unwrap();
        done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // The permit is free again and a new call still runs, so the dropped waiter really was cancelled.
        block_on(run(&limit, || Ok(()))).unwrap();
        assert_eq!(ran.load(Ordering::SeqCst), 1);

        // On a real database: a started update whose future is dropped still commits.
        let db = Database::in_memory().with_concurrency(1);
        let mut update = Box::pin(db.update("INSERT DATA { <urn:x> <urn:p> <urn:y> }"));
        let _ = update.as_mut().poll(&mut cx); // a free permit: the work starts within this poll
        drop(update);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !ask(&block_on(db.query_json("ASK { <urn:x> <urn:p> <urn:y> }")).unwrap()) {
            assert!(std::time::Instant::now() < deadline, "a started update was lost");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// An in-memory database of `n` triples with distinct subjects: a three-way cross product over it runs for far
    /// longer than any test waits.
    fn big(n: usize) -> Database {
        let db = Database::in_memory().with_concurrency(1);
        let data: String = (0..n).map(|i| format!("<urn:s{i}> <urn:p> \"{i}\" .\n")).collect();
        block_on(db.update(&format!("INSERT DATA {{ {data} }}"))).unwrap();
        db
    }
    const ENDLESS: &str = "SELECT (COUNT(*) AS ?n) { ?a ?p ?b . ?c ?q ?d . ?e ?r ?f }";

    /// Dropping a query's future after its work started stops the query: with one permit, the next call can only
    /// run once the worker has given it back, and it does so promptly.
    #[test]
    fn dropping_a_running_query_cancels_it() {
        let db = big(2000);
        let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        for json in [false, true] {
            let mut running: std::pin::Pin<Box<dyn Future<Output = bool>>> = if json {
                Box::pin(async { db.query_json(ENDLESS).await.is_ok() })
            } else {
                let snapshot = block_on(db.read()).unwrap();
                Box::pin(async move { snapshot.query(ENDLESS).await.is_ok() })
            };
            assert!(running.as_mut().poll(&mut cx).is_pending());
            std::thread::sleep(Duration::from_millis(50)); // well into the cross product
            drop(running);
            let t = std::time::Instant::now();
            assert!(ask(&block_on(db.query_json("ASK { <urn:s1> <urn:p> \"1\" }")).unwrap()));
            assert!(t.elapsed() < Duration::from_secs(5), "json={json}: the dropped query kept its worker for {:?}", t.elapsed());
        }
    }

    /// A timeout and a memory budget end a query with their own errors, and the database serves the next query.
    #[test]
    fn timeout_and_memory_budget() {
        let db = big(2000);
        let timeout = QueryOptions { timeout: Some(Duration::from_millis(50)), ..Default::default() };
        let t = std::time::Instant::now();
        let err = block_on(db.query_with(ENDLESS, timeout)).unwrap_err();
        assert!(matches!(err, DatabaseError::Interrupted { timed_out: true }), "{err}");
        assert!(t.elapsed() < Duration::from_secs(5), "took {:?}", t.elapsed());
        let budget = QueryOptions { memory_bytes: Some(1024), ..Default::default() };
        let grouped = "SELECT ?a (COUNT(*) AS ?n) { ?a ?p ?b } GROUP BY ?a";
        let err = block_on(db.query_json_with(grouped, budget)).unwrap_err();
        assert!(matches!(err, DatabaseError::ResourceLimit(_)), "{err}");
        // Room enough, and no limits at all: the same query answers.
        let roomy = QueryOptions { memory_bytes: Some(1 << 30), timeout: Some(Duration::from_secs(60)), ..Default::default() };
        assert!(block_on(db.query_json_with(grouped, roomy)).unwrap().contains("urn:s1999"));
        assert!(block_on(db.query_json(grouped)).unwrap().contains("urn:s0"));
    }

    /// Many concurrent calls complete, and no more than the limit run at once.
    #[test]
    fn concurrency_is_bounded() {
        let limit = Arc::new(Semaphore::new(3));
        let (now, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let futures: Vec<_> = (0..64).map(|i| {
            let (now, peak) = (Arc::clone(&now), Arc::clone(&peak));
            run(&limit, move || {
                let n = now.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(n, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(2));
                now.fetch_sub(1, Ordering::SeqCst);
                Ok(i)
            })
        }).collect();
        // Poll all 64 together (a future only runs once polled), until every one is ready.
        let mut pending: Vec<_> = futures.into_iter().map(|f| Some(Box::pin(f))).collect();
        let mut results = vec![None; 64];
        let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        while results.iter().any(Option::is_none) {
            for (i, slot) in pending.iter_mut().enumerate() {
                if let Some(f) = slot && let Poll::Ready(v) = f.as_mut().poll(&mut cx) { results[i] = Some(v.unwrap()); *slot = None }
            }
            if results.iter().any(Option::is_none) { std::thread::park_timeout(Duration::from_millis(50)) }
        }
        assert_eq!(results.into_iter().map(Option::unwrap).collect::<Vec<usize>>(), (0..64).collect::<Vec<_>>());
        let peak = peak.load(Ordering::SeqCst);
        assert!((2..=3).contains(&peak), "peak {peak}: the limit is 3, and polling them together must overlap some");
    }
}
