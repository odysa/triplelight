//! Python bindings for the embedded TripleLight database (`import triplelight`).
//! Terms are converted to Python values in Rust (see `to_py`). Query and update calls release the GIL.
use triplelight::rdf::Term;
use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use triplelight::embedded::{Database, DatabaseError, ReadSnapshot};
use triplelight::exec::{Entailment, QueryResult};
use std::sync::Arc;

create_exception!(_triplelight, TripleLightError, PyException);
create_exception!(_triplelight, SyntaxError, TripleLightError);
create_exception!(_triplelight, UnsupportedError, TripleLightError);
create_exception!(_triplelight, StorageError, TripleLightError);
create_exception!(_triplelight, LockedError, TripleLightError);
create_exception!(_triplelight, GraphStateError, TripleLightError);

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
    }
}

/// Python classes from `triplelight/__init__.py`, looked up once per call.
struct Classes<'py> { iri: Bound<'py, PyAny>, bnode: Bound<'py, PyAny>, literal: Bound<'py, PyAny>, triple: Bound<'py, PyAny> }
impl<'py> Classes<'py> {
    fn get(py: Python<'py>) -> PyResult<Self> {
        let m = py.import("triplelight")?;
        Ok(Classes { iri: m.getattr("IRI")?, bnode: m.getattr("BNode")?, literal: m.getattr("Literal")?, triple: m.getattr("Triple")? })
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
    #[pyo3(signature = (sparql, entailment="simple"))]
    fn query(&self, py: Python<'_>, sparql: &str, entailment: &str) -> PyResult<PyObject> {
        let (db, e) = (Arc::clone(&self.db), self::entailment(entailment)?);
        let r = py.allow_threads(|| db.query_with_entailment(sparql, e)).map_err(err)?;
        convert(py, r)
    }
    fn update(&self, py: Python<'_>, sparql: &str) -> PyResult<usize> {
        let db = Arc::clone(&self.db);
        py.allow_threads(|| db.update(sparql)).map_err(err)
    }
    /// SPARQL 1.1 JSON results as a string (fast path: terms written straight from the dictionary).
    #[pyo3(signature = (sparql, entailment="simple"))]
    fn query_json(&self, py: Python<'_>, sparql: &str, entailment: &str) -> PyResult<String> {
        let (db, e) = (Arc::clone(&self.db), self::entailment(entailment)?);
        py.allow_threads(|| db.query_json_with_entailment(sparql, e)).map_err(err)
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
    #[pyo3(signature = (sparql, entailment="simple"))]
    fn query(&self, py: Python<'_>, sparql: &str, entailment: &str) -> PyResult<PyObject> {
        let (s, e) = (Arc::clone(&self.snap), self::entailment(entailment)?);
        let r = py.allow_threads(|| s.query_with_entailment(sparql, e)).map_err(err)?;
        convert(py, r)
    }
    #[pyo3(signature = (sparql, entailment="simple"))]
    fn query_json(&self, py: Python<'_>, sparql: &str, entailment: &str) -> PyResult<String> {
        let (s, e) = (Arc::clone(&self.snap), self::entailment(entailment)?);
        py.allow_threads(|| s.query_json_with_entailment(sparql, e)).map_err(err)
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
