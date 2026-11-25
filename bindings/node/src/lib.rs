//! Node.js binding for the embedded TripleLight database. Query evaluation, updates, loading, compaction and
//! `Database.open` run on libuv's worker threads (napi `AsyncTask`); turning a result into JS objects happens on the
//! event loop when the Promise resolves. Terms are plain objects shaped like the RDF/JS data model (`termType`,
//! `value`, `language`, `datatype`), without RDF/JS `equals` methods.
use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi::{Env, JsObject, Task};
use napi_derive::napi;
use triplelight::rdf::{Literal, Term};
use triplelight::embedded::{Database as Db, DatabaseError, DatabaseOptions, ReadSnapshot};
use triplelight::exec::{Entailment, MemoryBudget, QueryLimits, QueryResult};

/// Errors cross into JS as `Error` objects whose message starts with the error kind; `index.js` turns them into
/// `TripleLightError` subclasses with a `kind` property.
fn err(e: DatabaseError) -> Error {
    let kind = match &e {
        DatabaseError::Syntax(_) => "SyntaxError",
        DatabaseError::Unsupported(_) => "UnsupportedError",
        DatabaseError::Storage(_) => "StorageError",
        DatabaseError::Locked => "LockedError",
        DatabaseError::GraphState(_) => "GraphStateError",
        DatabaseError::Interrupted { .. } => "InterruptedError",
        DatabaseError::ResourceLimit(_) => "ResourceLimitError",
    };
    Error::new(Status::GenericFailure, format!("{kind}: {e}"))
}

/// The `entailment` query option (#108): "simple" (the default) or "rdfs".
fn entailment(name: Option<String>) -> Result<Entailment> {
    match name.as_deref() {
        None | Some("simple") => Ok(Entailment::Simple),
        Some("rdfs") => Ok(Entailment::Rdfs),
        Some(other) => Err(Error::new(Status::InvalidArg, format!("entailment must be \"simple\" or \"rdfs\", not {other:?}"))),
    }
}

fn closed() -> Error { Error::new(Status::GenericFailure, "StorageError: the database is closed".to_string()) }

/// A snapshot keeps its database (and so the directory lock) alive: closing the `Database` handle while a snapshot
/// or a query on it is in flight must not let another process open and change the directory under it.
#[derive(Clone)]
enum Source { Db(Arc<Db>), Snap(Arc<Db>, Arc<ReadSnapshot>) }

impl Source {
    fn query(&self, q: &str, e: Entailment, limits: &Limits) -> std::result::Result<QueryResult, DatabaseError> {
        limits.with(|l| match self { Source::Db(d) => d.query_with_limits(q, e, l), Source::Snap(_, s) => s.query_with_limits(q, e, l) })
    }
    fn query_json(&self, q: &str, e: Entailment, limits: &Limits) -> std::result::Result<String, DatabaseError> {
        limits.with(|l| match self { Source::Db(d) => d.query_json_with_limits(q, e, l), Source::Snap(_, s) => s.query_json_with_limits(q, e, l) })
    }
}

/// Stops queries from outside them (#110): `index.js` makes one per query with a `timeoutMs` or a `signal` option,
/// and aborting the signal calls `cancel`. Cancelling is final.
#[napi]
pub struct Interrupt(Arc<triplelight::exec::Interrupt>);

#[napi]
impl Interrupt {
    /// `timeoutMs`: stop that many milliseconds from now; none: only when cancelled.
    #[napi(constructor)]
    pub fn new(timeout_ms: Option<f64>) -> Result<Self> {
        use triplelight::exec::Interrupt as I;
        let interrupt = match timeout_ms {
            None => I::new(),
            Some(ms) if ms.is_finite() && ms >= 0.0 => I::with_timeout(std::time::Duration::from_secs_f64(ms / 1000.0)),
            Some(ms) => return Err(Error::new(Status::InvalidArg, format!("timeoutMs must be a non-negative number, not {ms}"))),
        };
        Ok(Interrupt(Arc::new(interrupt)))
    }
    /// Stops every query running, or later run, with this interrupt. Safe at any time.
    #[napi]
    pub fn cancel(&self) { self.0.cancel() }
}

/// The limits of one query: an interrupt and a working-memory budget in megabytes.
#[derive(Default)]
pub struct Limits { interrupt: Option<Arc<triplelight::exec::Interrupt>>, memory_mb: Option<u32> }

impl Limits {
    fn new(interrupt: Option<&Interrupt>, memory_mb: Option<u32>) -> Result<Self> {
        if memory_mb == Some(0) { return Err(Error::new(Status::InvalidArg, "memoryMb must be positive".to_string())) }
        Ok(Limits { interrupt: interrupt.map(|i| Arc::clone(&i.0)), memory_mb })
    }
    fn with<T>(&self, f: impl FnOnce(QueryLimits<'_>) -> T) -> T {
        let budget = self.memory_mb.map(|mb| MemoryBudget::spilling(u64::from(mb) << 20));
        f(QueryLimits { interrupt: self.interrupt.as_deref(), memory: budget.as_ref() })
    }
}

// ---------- term and result conversion ----------

fn named(env: &Env, iri: &str) -> Result<JsObject> {
    let mut o = env.create_object()?;
    o.set_named_property("termType", env.create_string("NamedNode")?)?;
    o.set_named_property("value", env.create_string(iri)?)?;
    Ok(o)
}

fn literal(env: &Env, l: &Literal) -> Result<JsObject> {
    let mut o = env.create_object()?;
    o.set_named_property("termType", env.create_string("Literal")?)?;
    o.set_named_property("value", env.create_string(l.value())?)?;
    o.set_named_property("language", env.create_string(l.language().unwrap_or(""))?)?;
    o.set_named_property("datatype", named(env, l.datatype().as_str())?)?;
    // RDF 1.2 base direction: "ltr" or "rtl"
    // set only when present, so a build with the feature on keeps the exact 1.1 shape for every other literal
    #[cfg(feature = "rdf-12")]
    if let Some(dir) = l.direction() {
        let dir = match dir { triplelight::rdf::BaseDirection::Ltr => "ltr", triplelight::rdf::BaseDirection::Rtl => "rtl" };
        o.set_named_property("direction", env.create_string(dir)?)?;
    }
    Ok(o)
}

fn term(env: &Env, t: &Term) -> Result<JsObject> {
    match t {
        Term::NamedNode(n) => named(env, n.as_str()),
        Term::BlankNode(b) => {
            let mut o = env.create_object()?;
            o.set_named_property("termType", env.create_string("BlankNode")?)?;
            o.set_named_property("value", env.create_string(b.as_str())?)?;
            Ok(o)
        }
        Term::Literal(l) => literal(env, l),
        // RDF 1.2 triple term: an RDF/JS Quad term in the default graph, parts converted recursively
        #[cfg(feature = "rdf-12")]
        Term::Triple(t) => {
            let mut o = env.create_object()?;
            o.set_named_property("termType", env.create_string("Quad")?)?;
            o.set_named_property("value", env.create_string("")?)?;
            let subject: Term = t.subject.clone().into();
            let predicate: Term = t.predicate.clone().into();
            o.set_named_property("subject", term(env, &subject)?)?;
            o.set_named_property("predicate", term(env, &predicate)?)?;
            o.set_named_property("object", term(env, &t.object)?)?;
            let mut g = env.create_object()?;
            g.set_named_property("termType", env.create_string("DefaultGraph")?)?;
            g.set_named_property("value", env.create_string("")?)?;
            o.set_named_property("graph", g)?;
            Ok(o)
        }
        #[allow(unreachable_patterns)]
        _ => Err(Error::new(Status::GenericFailure, "UnsupportedError: unsupported term kind".to_string())),
    }
}

/// An object without a prototype, so a SPARQL variable named `__proto__` (or `constructor`, ...) is stored as an own
/// property instead of reaching `Object.prototype`.
fn bare_object(env: &Env, create: &napi::JsFunction) -> Result<JsObject> {
    create.call(None, &[env.get_null()?])?.coerce_to_object()
}

/// `{ type: "bindings", variables, bindings }`, `{ type: "boolean", boolean }` or `{ type: "triples", triples }`.
fn convert(env: &Env, r: QueryResult) -> Result<JsObject> {
    let object: JsObject = env.get_global()?.get_named_property_unchecked("Object")?;
    let create: napi::JsFunction = object.get_named_property("create")?;
    let mut out = env.create_object()?;
    match r {
        QueryResult::Solutions { vars, rows } => {
            out.set_named_property("type", env.create_string("bindings")?)?;
            let mut names = env.create_array_with_length(vars.len())?;
            for (i, v) in vars.iter().enumerate() { names.set_element(i as u32, env.create_string(v.as_str())?)?; }
            out.set_named_property("variables", names)?;
            let mut list = env.create_array_with_length(rows.len())?;
            for (i, row) in rows.iter().enumerate() {
                let mut b = bare_object(env, &create)?;
                for (v, t) in vars.iter().zip(row) {
                    if let Some(t) = t { b.set_named_property(v.as_str(), term(env, t)?)?; }
                }
                list.set_element(i as u32, b)?;
            }
            out.set_named_property("bindings", list)?;
        }
        QueryResult::Boolean(x) => {
            out.set_named_property("type", env.create_string("boolean")?)?;
            out.set_named_property("boolean", env.get_boolean(x)?)?;
        }
        QueryResult::Graph(triples) => {
            out.set_named_property("type", env.create_string("triples")?)?;
            let mut list = env.create_array_with_length(triples.len())?;
            for (i, t) in triples.iter().enumerate() {
                let mut o = env.create_object()?;
                o.set_named_property("subject", term(env, &t.subject.clone().into())?)?;
                o.set_named_property("predicate", named(env, t.predicate.as_str())?)?;
                o.set_named_property("object", term(env, &t.object)?)?;
                list.set_element(i as u32, o)?;
            }
            out.set_named_property("triples", list)?;
        }
        QueryResult::Ids(_) => return Err(Error::new(Status::GenericFailure, "StorageError: internal: undecoded result".to_string())),
    }
    Ok(out)
}

// ---------- SHACL ----------

/// SHACL Core validation (#108) of a data document against a shapes document, each "turtle" (the default) or
/// "ntriples". Resolves synchronously to `{ conforms, triples }`, the report graph. A syntax error is a `SyntaxError`,
/// shapes SHACL rejects a `ShapesError`, a feature this build lacks an `UnsupportedError`; violations are a report.
#[napi(ts_return_type = "ValidationReport")]
pub fn validate(env: Env, data: String, shapes: String, data_format: Option<String>, shapes_format: Option<String>) -> Result<JsObject> {
    #[cfg(not(feature = "shacl"))]
    {
        let _ = (env, data, shapes, data_format, shapes_format);
        Err(Error::new(Status::GenericFailure, "UnsupportedError: SHACL validation requires the shacl Cargo feature".to_owned()))
    }
    #[cfg(feature = "shacl")]
    {
        use triplelight::shacl::{DocumentError, ValidationError};
        let format = |name: Option<String>| {
            let name = name.unwrap_or_else(|| "turtle".into());
            triplelight::rdf::Format::from_name(&name)
                .ok_or_else(|| Error::new(Status::InvalidArg, format!("format must be \"turtle\" or \"ntriples\", not {name:?}")))
        };
        let (df, sf) = (format(data_format)?, format(shapes_format)?);
        let report = triplelight::shacl::validate_documents(data.as_bytes(), df, shapes.as_bytes(), sf).map_err(|e| {
            let kind = match &e {
                DocumentError::Syntax { .. } => "SyntaxError",
                DocumentError::Validation(ValidationError::Unsupported(_)) => "UnsupportedError",
                DocumentError::Validation(_) => "ShapesError",
            };
            Error::new(Status::GenericFailure, format!("{kind}: {e}"))
        })?;
        let mut out = convert(&env, QueryResult::Graph(report.to_triples()))?;
        out.set_named_property("conforms", env.get_boolean(report.conforms)?)?;
        Ok(out)
    }
}

// ---------- background tasks ----------

pub struct QueryTask { src: Source, sparql: String, entailment: Entailment, limits: Limits }
impl Task for QueryTask {
    type Output = QueryResult;
    type JsValue = JsObject;
    fn compute(&mut self) -> Result<Self::Output> { self.src.query(&self.sparql, self.entailment, &self.limits).map_err(err) }
    fn resolve(&mut self, env: Env, out: Self::Output) -> Result<Self::JsValue> { convert(&env, out) }
}

pub struct JsonTask { src: Source, sparql: String, entailment: Entailment, limits: Limits }
impl Task for JsonTask {
    type Output = String;
    type JsValue = String;
    fn compute(&mut self) -> Result<Self::Output> { self.src.query_json(&self.sparql, self.entailment, &self.limits).map_err(err) }
    fn resolve(&mut self, _: Env, out: Self::Output) -> Result<Self::JsValue> { Ok(out) }
}

pub struct OpenTask { path: String, initial: Option<String> }
impl Task for OpenTask {
    type Output = Db;
    type JsValue = Database;
    fn compute(&mut self) -> Result<Self::Output> {
        let mut opts = DatabaseOptions::default();
        opts.initial = self.initial.take().map(Into::into);
        Db::open_with(&self.path, opts).map_err(err)
    }
    fn resolve(&mut self, _: Env, out: Self::Output) -> Result<Self::JsValue> { Ok(Database { db: Some(Arc::new(out)) }) }
}

pub enum Write { Update(String), Load(String), Compact }
pub struct WriteTask { db: Arc<Db>, op: Write }
impl Task for WriteTask {
    type Output = usize;
    type JsValue = f64;
    fn compute(&mut self) -> Result<Self::Output> {
        match &self.op {
            Write::Update(q) => self.db.update(q).map_err(err),
            Write::Load(p) => self.db.load(p).map_err(err),
            Write::Compact => self.db.compact().map(|_| 0).map_err(err),
        }
    }
    fn resolve(&mut self, _: Env, out: Self::Output) -> Result<Self::JsValue> { Ok(out as f64) }
}

// ---------- classes ----------

/// A persistent RDF database in a local directory (created if missing). One process may open a directory at a time.
#[napi]
pub struct Database { db: Option<Arc<Db>> }

#[napi]
impl Database {
    /// Opens (or creates) the database at `path`. `initial` is an N-Triples file loaded only into a new database.
    #[napi(constructor)]
    pub fn new(path: String, initial: Option<String>) -> Result<Self> {
        let mut opts = DatabaseOptions::default();
        opts.initial = initial.map(Into::into);
        Ok(Database { db: Some(Arc::new(Db::open_with(&path, opts).map_err(err)?)) })
    }
    /// Creates a non-durable in-memory database.
    #[napi(factory)]
    pub fn in_memory() -> Self { Database { db: Some(Arc::new(Db::in_memory())) } }
    /// Opens the database on a worker thread (recovery and the initial load can take a while).
    #[napi(ts_return_type = "Promise<Database>")]
    pub fn open(path: String, initial: Option<String>) -> AsyncTask<OpenTask> {
        AsyncTask::new(OpenTask { path, initial })
    }
    fn db(&self) -> Result<Arc<Db>> { self.db.clone().ok_or_else(closed) }

    /// Runs a SPARQL SELECT / ASK / CONSTRUCT / DESCRIBE query on the current state. `entailment` "rdfs" matches the
    /// default graph's RDFS closure, computed at query time; the default is "simple".
    /// `interrupt` and `memoryMb` limit it (#110).
    #[napi(ts_return_type = "Promise<QueryResult>")]
    pub fn query(&self, sparql: String, entailment: Option<String>, interrupt: Option<&Interrupt>, memory_mb: Option<u32>) -> Result<AsyncTask<QueryTask>> {
        let limits = Limits::new(interrupt, memory_mb)?;
        Ok(AsyncTask::new(QueryTask { src: Source::Db(self.db()?), sparql, entailment: self::entailment(entailment)?, limits }))
    }
    /// Like `query`, but blocks the calling thread (for scripts and tests).
    #[napi(ts_return_type = "QueryResult")]
    pub fn query_sync(&self, env: Env, sparql: String, entailment: Option<String>, interrupt: Option<&Interrupt>, memory_mb: Option<u32>) -> Result<JsObject> {
        let limits = Limits::new(interrupt, memory_mb)?;
        let r = Source::Db(self.db()?).query(&sparql, self::entailment(entailment)?, &limits).map_err(err)?;
        convert(&env, r)
    }
    /// SPARQL 1.1 Query Results JSON as a string: the fastest path for large results.
    #[napi(ts_return_type = "Promise<string>")]
    pub fn query_json(&self, sparql: String, entailment: Option<String>, interrupt: Option<&Interrupt>, memory_mb: Option<u32>) -> Result<AsyncTask<JsonTask>> {
        let limits = Limits::new(interrupt, memory_mb)?;
        Ok(AsyncTask::new(JsonTask { src: Source::Db(self.db()?), sparql, entailment: self::entailment(entailment)?, limits }))
    }
    /// Applies a SPARQL Update atomically and durably (fsync before the Promise resolves). Resolves to the number of
    /// triples changed.
    #[napi(ts_return_type = "Promise<number>")]
    pub fn update(&self, sparql: String) -> Result<AsyncTask<WriteTask>> {
        Ok(AsyncTask::new(WriteTask { db: self.db()?, op: Write::Update(sparql) }))
    }
    /// Atomically imports an N-Triples (.nt), Turtle (.ttl), N-Quads (.nq) or TriG (.trig) file. Resolves to the number of triples added.
    #[napi(ts_return_type = "Promise<number>")]
    pub fn load(&self, path: String) -> Result<AsyncTask<WriteTask>> {
        Ok(AsyncTask::new(WriteTask { db: self.db()?, op: Write::Load(path) }))
    }
    /// Merges updates into a new immutable base; open snapshots keep their view.
    #[napi(ts_return_type = "Promise<void>")]
    pub fn compact(&self) -> Result<AsyncTask<WriteTask>> {
        Ok(AsyncTask::new(WriteTask { db: self.db()?, op: Write::Compact }))
    }
    /// Loads an official extension for this database.
    #[napi]
    pub fn load_extension(&self, name: String) -> Result<()> {
        self.db()?.load_extension(&name)
            .map_err(|error| Error::new(Status::GenericFailure, format!("UnsupportedError: {error}")))
    }

    #[napi(ts_return_type = "ExtensionInfo[]")]
    pub fn list_extensions(&self, env: Env) -> Result<Vec<JsObject>> {
        self.db()?.list_extensions().into_iter().map(|entry| {
            let mut object = env.create_object()?;
            object.set_named_property("name", env.create_string(&entry.name)?)?;
            object.set_named_property("compiledIn", env.get_boolean(entry.compiled_in)?)?;
            object.set_named_property("loaded", env.get_boolean(entry.loaded)?)?;
            Ok(object)
        }).collect()
    }

    /// True once background optimizer statistics are built (queries work before that, with simpler plans).
    #[napi]
    pub fn stats_ready(&self) -> Result<bool> { Ok(self.db()?.stats_ready()) }
    /// A consistent, immutable view for several queries; later updates do not change it.
    #[napi]
    pub fn read(&self) -> Result<Snapshot> { let db = self.db()?; Ok(Snapshot { snap: Arc::new(db.read()), db }) }
    /// Releases this handle. The directory lock is released once no snapshot or pending operation uses it.
    #[napi]
    pub fn close(&mut self) { self.db = None; }
}

/// An immutable view of the database, unaffected by later updates.
#[napi]
pub struct Snapshot { db: Arc<Db>, snap: Arc<ReadSnapshot> }

#[napi]
impl Snapshot {
    #[napi(ts_return_type = "Promise<QueryResult>")]
    pub fn query(&self, sparql: String, entailment: Option<String>, interrupt: Option<&Interrupt>, memory_mb: Option<u32>) -> Result<AsyncTask<QueryTask>> {
        let limits = Limits::new(interrupt, memory_mb)?;
        Ok(AsyncTask::new(QueryTask { src: self.source(), sparql, entailment: self::entailment(entailment)?, limits }))
    }
    #[napi(ts_return_type = "QueryResult")]
    pub fn query_sync(&self, env: Env, sparql: String, entailment: Option<String>, interrupt: Option<&Interrupt>, memory_mb: Option<u32>) -> Result<JsObject> {
        let limits = Limits::new(interrupt, memory_mb)?;
        let r = self.source().query(&sparql, self::entailment(entailment)?, &limits).map_err(err)?;
        convert(&env, r)
    }
    #[napi(ts_return_type = "Promise<string>")]
    pub fn query_json(&self, sparql: String, entailment: Option<String>, interrupt: Option<&Interrupt>, memory_mb: Option<u32>) -> Result<AsyncTask<JsonTask>> {
        let limits = Limits::new(interrupt, memory_mb)?;
        Ok(AsyncTask::new(JsonTask { src: self.source(), sparql, entailment: self::entailment(entailment)?, limits }))
    }
}

impl Snapshot {
    fn source(&self) -> Source { Source::Snap(Arc::clone(&self.db), Arc::clone(&self.snap)) }
}
