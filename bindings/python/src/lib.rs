//! Python bindings for the embedded TripleLight database (`import triplelight`).
//! Terms are converted to Python values in Rust (see `to_py`). Query and update calls release the GIL.
use triplelight::rdf::Term;
use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use triplelight::embedded::{Database, DatabaseError, ReadSnapshot};
use triplelight::exec::{Entailment, Interrupt, MemoryBudget, QueryLimits, QueryResult};
use triplelight::extension::FunctionOptions;
use std::sync::Arc;

mod scalar;

create_exception!(_triplelight, TripleLightError, PyException);
create_exception!(_triplelight, SyntaxError, TripleLightError);
create_exception!(_triplelight, UnsupportedError, TripleLightError);
create_exception!(_triplelight, StorageError, TripleLightError);
create_exception!(_triplelight, LockedError, TripleLightError);
create_exception!(_triplelight, GraphStateError, TripleLightError);
create_exception!(_triplelight, ShapesError, TripleLightError);
create_exception!(_triplelight, QueryCancelled, TripleLightError);
create_exception!(_triplelight, QueryTimeout, TripleLightError);
create_exception!(_triplelight, ResourceLimitError, TripleLightError);

/// `entailment="simple"|"rdfs"` of the query methods (#108); anything else is a `ValueError`.
fn entailment(name: &str) -> PyResult<Entailment> {
    match name {
        "simple" => Ok(Entailment::Simple),
        "rdfs" => Ok(Entailment::Rdfs),
        other => Err(pyo3::exceptions::PyValueError::new_err(format!("entailment must be \"simple\" or \"rdfs\", not {other:?}"))),
    }
}

fn err(e: DatabaseError) -> PyErr {
    match e {
        DatabaseError::Syntax(m) => SyntaxError::new_err(m),
        DatabaseError::Unsupported(m) => UnsupportedError::new_err(m),
        DatabaseError::Storage(e) => StorageError::new_err(e.to_string()),
        DatabaseError::Locked => LockedError::new_err("database directory is already open by another instance"),
        DatabaseError::GraphState(m) => GraphStateError::new_err(m),
        DatabaseError::Interrupted { timed_out: true } => QueryTimeout::new_err("query timed out"),
        DatabaseError::Interrupted { timed_out: false } => QueryCancelled::new_err("query cancelled"),
        DatabaseError::ResourceLimit(m) => ResourceLimitError::new_err(m),
    }
}

/// Ctrl-C cancels a query running on the main thread (#110). The SIGINT action is installed on the first such
/// query, never at import, and is chained by `signal-hook`, so Python's own handler (or the host's) still runs and
/// raises `KeyboardInterrupt` as usual. The action only sets a flag, which the running query's interrupt watches.
mod ctrl_c {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, OnceLock};

    static ENABLED: AtomicBool = AtomicBool::new(true);
    static FLAG: OnceLock<Option<Arc<AtomicBool>>> = OnceLock::new();

    /// `set_cancel_on_ctrl_c`: off, no action is installed (or, once installed, queries stop watching it).
    pub fn set_enabled(enabled: bool) { ENABLED.store(enabled, Ordering::SeqCst) }

    /// The flag a main-thread query watches, cleared for this query: installs the action on first use.
    pub fn flag() -> Option<Arc<AtomicBool>> {
        if !ENABLED.load(Ordering::SeqCst) { return None }
        let flag = FLAG.get_or_init(|| {
            let flag = Arc::new(AtomicBool::new(false));
            #[cfg(unix)]
            { signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&flag)).ok().map(|_| flag) }
            #[cfg(not(unix))]
            { let _ = flag; None }
        }).clone()?;
        // A press before this query belongs to Python, which still raises it.
        flag.store(false, Ordering::SeqCst);
        Some(flag)
    }
}

/// Runs a query `f` under `timeout` (seconds) and `memory_mb` without the GIL (#110). On the main thread, Ctrl-C
/// cancels it (`ctrl_c`) and the `KeyboardInterrupt` is raised once it has stopped.
fn limited<T: Send>(
    py: Python<'_>,
    timeout: Option<f64>,
    memory_mb: Option<u64>,
    spill: bool,
    f: impl FnOnce(QueryLimits<'_>) -> Result<T, DatabaseError> + Send,
) -> PyResult<T> {
    let mut interrupt = match timeout {
        None => Interrupt::new(),
        Some(t) if t.is_finite() && t >= 0.0 => Interrupt::with_timeout(std::time::Duration::from_secs_f64(t)),
        Some(t) => return Err(pyo3::exceptions::PyValueError::new_err(format!("timeout must be a non-negative number of seconds, not {t}"))),
    };
    let budget = match memory_mb {
        None => None,
        Some(0) => return Err(pyo3::exceptions::PyValueError::new_err("memory_mb must be positive")),
        // `spill=True`: GROUP BY over the budget spills to TRIPLELIGHT_SCRATCH_DIR (else the temp directory) (#111)
        Some(mb) if spill => Some(MemoryBudget::spilling(mb.saturating_mul(1 << 20))),
        Some(mb) => Some(MemoryBudget::new(mb.saturating_mul(1 << 20))),
    };
    let mut watching = false;
    if on_main_thread(py)? && let Some(flag) = ctrl_c::flag() {
        interrupt = interrupt.with_flag(flag);
        watching = true;
    }
    let limits = QueryLimits { interrupt: Some(&interrupt), memory: budget.as_ref() };
    let result = py.allow_threads(|| f(limits));
    if watching && matches!(result, Err(DatabaseError::Interrupted { timed_out: false })) { py.check_signals()? }
    result.map_err(err)
}

/// Whether this is Python's main thread, the only one signals reach.
fn on_main_thread(py: Python<'_>) -> PyResult<bool> {
    static MAIN: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let threading = py.import("threading")?;
    let main = match MAIN.get() {
        Some(&id) => id,
        None => *MAIN.get_or_init(|| threading.call_method0("main_thread").and_then(|t| t.getattr("ident")).and_then(|i| i.extract()).unwrap_or(0)),
    };
    Ok(threading.call_method0("get_ident")?.extract::<u64>()? == main)
}

#[pyfunction]
/// Whether Ctrl-C cancels a query running on the main thread (default True). Turning it off before the first query
/// means triplelight never installs a SIGINT action, for hosts that manage SIGINT themselves.
fn set_cancel_on_ctrl_c(enabled: bool) { ctrl_c::set_enabled(enabled) }

/// Python classes from `triplelight/__init__.py`, looked up once per call.
struct Classes<'py> {
    iri: Bound<'py, PyAny>,
    bnode: Bound<'py, PyAny>,
    literal: Bound<'py, PyAny>,
    #[cfg(feature = "rdf-12")]
    triple: Bound<'py, PyAny>,
}
impl<'py> Classes<'py> {
    fn get(py: Python<'py>) -> PyResult<Self> {
        let m = py.import("triplelight")?;
        Ok(Classes {
            iri: m.getattr("IRI")?,
            bnode: m.getattr("BNode")?,
            literal: m.getattr("Literal")?,
            #[cfg(feature = "rdf-12")]
            triple: m.getattr("Triple")?,
        })
    }
}
const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";
const XSD_BOOLEAN: &str = "http://www.w3.org/2001/XMLSchema#boolean";

/// Lossless Python value of a term (same rules as the pure-Python mapping): IRI/BNode str subclasses, str for plain
/// literals, int only for canonical xsd:integer, bool for "true"/"false", otherwise Literal(value, datatype, lang).
fn to_py<'py>(py: Python<'py>, c: &Classes<'py>, t: &Term) -> PyResult<Bound<'py, PyAny>> {
    match t {
        Term::NamedNode(n) => c.iri.call1((n.as_str(),)),
        Term::BlankNode(b) => c.bnode.call1((b.as_str(),)),
        Term::Literal(l) => {
            if let Some(lang) = l.language() {
                // a base direction ("ltr"/"rtl", RDF 1.2) is the fourth Literal field; plain language tags keep three
                #[cfg(feature = "rdf-12")]
                if let Some(dir) = l.direction() {
                    let d = match dir { triplelight::rdf::BaseDirection::Ltr => "ltr", triplelight::rdf::BaseDirection::Rtl => "rtl" };
                    return c.literal.call1((l.value(), py.None(), lang, d))
                }
                return c.literal.call1((l.value(), py.None(), lang))
            }
            let dt = l.datatype().as_str();
            let v = l.value();
            if dt == XSD_STRING { return Ok(v.into_pyobject(py)?.into_any()) }
            if dt == XSD_INTEGER {
                if let Ok(i) = v.parse::<i64>() { if i.to_string() == v { return Ok(i.into_pyobject(py)?.into_any()) } }
            }
            if dt == XSD_BOOLEAN && (v == "true" || v == "false") { return Ok(pyo3::types::PyBool::new(py, v == "true").to_owned().into_any()) }
            c.literal.call1((v, dt, py.None()))
        }
        // RDF 1.2 triple term -> Triple(subject, predicate, object), recursive
        #[cfg(feature = "rdf-12")]
        Term::Triple(t) => {
            let s: Term = t.subject.clone().into();
            let p: Term = t.predicate.clone().into();
            c.triple.call1((to_py(py, c, &s)?, to_py(py, c, &p)?, to_py(py, c, &t.object)?))
        }
    }
}

/// ("select", vars, rows: list[tuple]) | ("ask", bool) | ("graph", list[tuple]) with final Python values.
fn convert(py: Python<'_>, r: QueryResult) -> PyResult<PyObject> {
    use pyo3::types::{PyList, PyTuple};
    let c = Classes::get(py)?;
    Ok(match r {
        QueryResult::Solutions { vars, rows } => {
            let vars: Vec<String> = vars.iter().map(|v| v.as_str().to_owned()).collect();
            let out = PyList::empty(py);
            for row in &rows {
                let cells: Vec<Bound<'_, PyAny>> = row.iter().map(|t| match t { Some(t) => to_py(py, &c, t), None => Ok(py.None().into_bound(py)) }).collect::<PyResult<_>>()?;
                out.append(PyTuple::new(py, cells)?)?;
            }
            ("select", vars, out).into_pyobject(py)?.into_any().unbind()
        }
        QueryResult::Boolean(b) => ("ask", b).into_pyobject(py)?.into_any().unbind(),
        QueryResult::Graph(ts) => {
            let out = PyList::empty(py);
            for t in &ts {
                let s: Term = t.subject.clone().into();
                let p: Term = t.predicate.clone().into();
                out.append(PyTuple::new(py, [to_py(py, &c, &s)?, to_py(py, &c, &p)?, to_py(py, &c, &t.object)?])?)?;
            }
            ("graph", out).into_pyobject(py)?.into_any().unbind()
        }
        QueryResult::Ids(_) => return Err(TripleLightError::new_err("internal: undecoded result")),
    })
}

#[pyclass(frozen, name = "Database", module = "triplelight._triplelight")]
struct PyDatabase { db: Arc<Database> }

#[pymethods]
impl PyDatabase {
    #[new]
    #[pyo3(signature = (path, initial=None))]
    fn new(py: Python<'_>, path: std::path::PathBuf, initial: Option<std::path::PathBuf>) -> PyResult<Self> {
        let mut opts = triplelight::embedded::DatabaseOptions::default();
        opts.initial = initial;
        let db = py.allow_threads(|| Database::open_with(&path, opts)).map_err(err)?;
        Ok(PyDatabase { db: Arc::new(db) })
    }
    /// Creates a non-durable in-memory database.
    #[staticmethod]
    fn in_memory() -> Self { Self { db: Arc::new(Database::in_memory()) } }
    /// `entailment="rdfs"` matches the default graph's RDFS closure, computed at query time; the default is "simple".
    /// `timeout` (seconds) raises `QueryTimeout`, `memory_mb` bounds the working memory (`ResourceLimitError`), and
    /// Ctrl-C cancels a running query.
    #[pyo3(signature = (sparql, entailment="simple", timeout=None, memory_mb=None, spill=false))]
    fn query(&self, py: Python<'_>, sparql: &str, entailment: &str, timeout: Option<f64>, memory_mb: Option<u64>, spill: bool) -> PyResult<PyObject> {
        let (db, e) = (Arc::clone(&self.db), self::entailment(entailment)?);
        let r = limited(py, timeout, memory_mb, spill, |limits| db.query_with_limits(sparql, e, limits))?;
        convert(py, r)
    }
    fn update(&self, py: Python<'_>, sparql: &str) -> PyResult<usize> {
        let db = Arc::clone(&self.db);
        py.allow_threads(|| db.update(sparql)).map_err(err)
    }
    /// SPARQL 1.1 JSON results as a string (fast path: terms written straight from the dictionary).
    #[pyo3(signature = (sparql, entailment="simple", timeout=None, memory_mb=None, spill=false))]
    fn query_json(&self, py: Python<'_>, sparql: &str, entailment: &str, timeout: Option<f64>, memory_mb: Option<u64>, spill: bool) -> PyResult<String> {
        let (db, e) = (Arc::clone(&self.db), self::entailment(entailment)?);
        limited(py, timeout, memory_mb, spill, |limits| db.query_json_with_limits(sparql, e, limits))
    }
    /// Atomically imports an N-Triples (.nt), Turtle (.ttl), N-Quads (.nq) or TriG (.trig) file; returns triples added.
    fn load(&self, py: Python<'_>, path: std::path::PathBuf) -> PyResult<usize> {
        let db = Arc::clone(&self.db);
        py.allow_threads(|| db.load(&path)).map_err(err)
    }
    fn load_extension(&self, name: &str) -> PyResult<()> {
        self.db.load_extension(name).map_err(|error| UnsupportedError::new_err(error.to_string()))
    }
    #[pyo3(signature = (iri, callback, *, arity=None, pure=false))]
    fn register_function(
        &self,
        iri: &str,
        callback: Bound<'_, PyAny>,
        arity: Option<usize>,
        pure: bool,
    ) -> PyResult<()> {
        if !callback.is_callable() {
            return Err(pyo3::exceptions::PyTypeError::new_err("callback must be callable"));
        }
        let callback = callback.unbind();
        self.db
            .register_function(iri, FunctionOptions { arity, pure }, move |args: &[Term]| {
                scalar::call(&callback, args)
            })
            .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))
    }
    fn list_extensions(&self) -> Vec<(String, bool, bool)> {
        self.db.list_extensions().into_iter().map(|entry| (entry.name, entry.compiled_in, entry.loaded)).collect()
    }
    fn stats_ready(&self) -> bool { self.db.stats_ready() }
    fn read(&self) -> PySnapshot { PySnapshot { snap: Arc::new(self.db.read()) } }
    fn compact(&self, py: Python<'_>) -> PyResult<()> {
        let db = Arc::clone(&self.db);
        py.allow_threads(|| db.compact()).map_err(err)
    }
}

#[pyclass(frozen, name = "Snapshot", module = "triplelight._triplelight")]
struct PySnapshot { snap: Arc<ReadSnapshot> }

#[pymethods]
impl PySnapshot {
    #[pyo3(signature = (sparql, entailment="simple", timeout=None, memory_mb=None, spill=false))]
    fn query(&self, py: Python<'_>, sparql: &str, entailment: &str, timeout: Option<f64>, memory_mb: Option<u64>, spill: bool) -> PyResult<PyObject> {
        let (s, e) = (Arc::clone(&self.snap), self::entailment(entailment)?);
        let r = limited(py, timeout, memory_mb, spill, |limits| s.query_with_limits(sparql, e, limits))?;
        convert(py, r)
    }
    #[pyo3(signature = (sparql, entailment="simple", timeout=None, memory_mb=None, spill=false))]
    fn query_json(&self, py: Python<'_>, sparql: &str, entailment: &str, timeout: Option<f64>, memory_mb: Option<u64>, spill: bool) -> PyResult<String> {
        let (s, e) = (Arc::clone(&self.snap), self::entailment(entailment)?);
        limited(py, timeout, memory_mb, spill, |limits| s.query_json_with_limits(sparql, e, limits))
    }
}

/// SHACL Core validation (#108) of a data document against a shapes document, each "turtle" or "ntriples". Returns
/// `(conforms, report)`, the report as a graph result. A syntax error raises `SyntaxError`; shapes SHACL rejects raise
/// `ShapesError`; a feature this build lacks raises `UnsupportedError`. Data that violates the shapes is a report.
#[pyfunction]
#[pyo3(signature = (data, shapes, data_format="turtle", shapes_format="turtle"))]
fn validate(py: Python<'_>, data: &str, shapes: &str, data_format: &str, shapes_format: &str) -> PyResult<(bool, PyObject)> {
    #[cfg(not(feature = "shacl"))]
    {
        let _ = (py, data, shapes, data_format, shapes_format);
        Err(UnsupportedError::new_err("SHACL validation requires the shacl Cargo feature"))
    }
    #[cfg(feature = "shacl")]
    {
        use triplelight::shacl::{DocumentError, ValidationError};
        let format = |name: &str| triplelight::rdf::Format::from_name(name)
            .ok_or_else(|| pyo3::exceptions::PyValueError::new_err(format!("format must be \"turtle\" or \"ntriples\", not {name:?}")));
        let (df, sf) = (format(data_format)?, format(shapes_format)?);
        let report = py.allow_threads(|| triplelight::shacl::validate_documents(data.as_bytes(), df, shapes.as_bytes(), sf)).map_err(|e| match &e {
            DocumentError::Syntax { .. } => SyntaxError::new_err(e.to_string()),
            DocumentError::Validation(ValidationError::Unsupported(_)) => UnsupportedError::new_err(e.to_string()),
            DocumentError::Validation(_) => ShapesError::new_err(e.to_string()),
        })?;
        Ok((report.conforms, convert(py, QueryResult::Graph(report.to_triples()))?))
    }
}

#[pymodule]
fn _triplelight(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(validate, m)?)?;
    m.add_function(wrap_pyfunction!(set_cancel_on_ctrl_c, m)?)?;
    m.add("ShapesError", m.py().get_type::<ShapesError>())?;
    m.add_class::<PyDatabase>()?;
    m.add_class::<PySnapshot>()?;
    m.add("TripleLightError", m.py().get_type::<TripleLightError>())?;
    m.add("SyntaxError", m.py().get_type::<SyntaxError>())?;
    m.add("UnsupportedError", m.py().get_type::<UnsupportedError>())?;
    m.add("GraphStateError", m.py().get_type::<GraphStateError>())?;
    m.add("StorageError", m.py().get_type::<StorageError>())?;
    m.add("LockedError", m.py().get_type::<LockedError>())?;
    m.add("QueryCancelled", m.py().get_type::<QueryCancelled>())?;
    m.add("QueryTimeout", m.py().get_type::<QueryTimeout>())?;
    m.add("ResourceLimitError", m.py().get_type::<ResourceLimitError>())?;
    Ok(())
}
