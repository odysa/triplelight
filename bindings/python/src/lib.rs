//! Python bindings for the embedded TripleLight database (`import triplelight`).
//! Terms are converted to Python values in Rust (see `to_py`). Query and update calls release the GIL.
use oxrdf::Term;
use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use triplelight::embedded::{Database, DatabaseError, ReadSnapshot};
use triplelight::exec::QueryResult;
use std::sync::Arc;

create_exception!(_triplelight, TripleLightError, PyException);
create_exception!(_triplelight, SyntaxError, TripleLightError);
create_exception!(_triplelight, UnsupportedError, TripleLightError);
create_exception!(_triplelight, StorageError, TripleLightError);
create_exception!(_triplelight, LockedError, TripleLightError);
create_exception!(_triplelight, GraphStateError, TripleLightError);

fn err(e: DatabaseError) -> PyErr {
    match e {
        DatabaseError::Syntax(m) => SyntaxError::new_err(m),
        DatabaseError::Unsupported(m) => UnsupportedError::new_err(m),
        DatabaseError::Storage(e) => StorageError::new_err(e.to_string()),
        DatabaseError::Locked => LockedError::new_err("database directory is already open by another instance"),
        DatabaseError::GraphState(m) => GraphStateError::new_err(m),
    }
}

/// Python classes from `triplelight/__init__.py`, looked up once per call.
struct Classes<'py> { iri: Bound<'py, PyAny>, bnode: Bound<'py, PyAny>, literal: Bound<'py, PyAny> }
impl<'py> Classes<'py> {
    fn get(py: Python<'py>) -> PyResult<Self> {
        let m = py.import("triplelight")?;
        Ok(Classes { iri: m.getattr("IRI")?, bnode: m.getattr("BNode")?, literal: m.getattr("Literal")? })
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
            if let Some(lang) = l.language() { return c.literal.call1((l.value(), py.None(), lang)) }
            let dt = l.datatype().as_str();
            let v = l.value();
            if dt == XSD_STRING { return Ok(v.into_pyobject(py)?.into_any()) }
            if dt == XSD_INTEGER {
                if let Ok(i) = v.parse::<i64>() { if i.to_string() == v { return Ok(i.into_pyobject(py)?.into_any()) } }
            }
            if dt == XSD_BOOLEAN && (v == "true" || v == "false") { return Ok(pyo3::types::PyBool::new(py, v == "true").to_owned().into_any()) }
            c.literal.call1((v, dt, py.None()))
        }
        #[allow(unreachable_patterns)]
        _ => Ok(py.None().into_bound(py)),
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
    fn query(&self, py: Python<'_>, sparql: &str) -> PyResult<PyObject> {
        let db = Arc::clone(&self.db);
        let r = py.allow_threads(|| db.query(sparql)).map_err(err)?;
        convert(py, r)
    }
    fn update(&self, py: Python<'_>, sparql: &str) -> PyResult<usize> {
        let db = Arc::clone(&self.db);
        py.allow_threads(|| db.update(sparql)).map_err(err)
    }
    /// SPARQL 1.1 JSON results as a string (fast path: terms written straight from the dictionary).
    fn query_json(&self, py: Python<'_>, sparql: &str) -> PyResult<String> {
        let db = Arc::clone(&self.db);
        py.allow_threads(|| db.query_json(sparql)).map_err(err)
    }
    /// Atomically imports an N-Triples (.nt), Turtle (.ttl), N-Quads (.nq) or TriG (.trig) file; returns triples added.
    fn load(&self, py: Python<'_>, path: std::path::PathBuf) -> PyResult<usize> {
        let db = Arc::clone(&self.db);
        py.allow_threads(|| db.load(&path)).map_err(err)
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
    fn query(&self, py: Python<'_>, sparql: &str) -> PyResult<PyObject> {
        let s = Arc::clone(&self.snap);
        let r = py.allow_threads(|| s.query(sparql)).map_err(err)?;
        convert(py, r)
    }
    fn query_json(&self, py: Python<'_>, sparql: &str) -> PyResult<String> {
        let s = Arc::clone(&self.snap);
        py.allow_threads(|| s.query_json(sparql)).map_err(err)
    }
}

#[pymodule]
fn _triplelight(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyDatabase>()?;
    m.add_class::<PySnapshot>()?;
    m.add("TripleLightError", m.py().get_type::<TripleLightError>())?;
    m.add("SyntaxError", m.py().get_type::<SyntaxError>())?;
    m.add("UnsupportedError", m.py().get_type::<UnsupportedError>())?;
    m.add("GraphStateError", m.py().get_type::<GraphStateError>())?;
    m.add("StorageError", m.py().get_type::<StorageError>())?;
    m.add("LockedError", m.py().get_type::<LockedError>())?;
    Ok(())
}
