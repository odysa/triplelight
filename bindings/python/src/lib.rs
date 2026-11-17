//! Python bindings for the embedded rdfdb database (`import rdfdb`).
//! Terms cross the boundary as plain tuples `(kind, value, datatype, lang)`; `rdfdb/__init__.py` turns them into
//! Python values. Query and update calls release the GIL.
use oxrdf::Term;
use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use rdfdb::embedded::{Database, ReadSnapshot};
use rdfdb::exec::{QueryError, QueryErrorKind, QueryResult};
use std::sync::Arc;

create_exception!(_rdfdb, RdfdbError, PyException);
create_exception!(_rdfdb, SyntaxError, RdfdbError);
create_exception!(_rdfdb, UnsupportedError, RdfdbError);
create_exception!(_rdfdb, StorageError, RdfdbError);

fn query_err(e: QueryError) -> PyErr {
    match e.kind {
        QueryErrorKind::Syntax => SyntaxError::new_err(e.message),
        QueryErrorKind::Unsupported => UnsupportedError::new_err(e.message),
        QueryErrorKind::Storage => StorageError::new_err(e.message),
    }
}
fn io_err(e: std::io::Error) -> PyErr { StorageError::new_err(e.to_string()) }

type RawTerm = (u8, String, Option<String>, Option<String>);
/// kind: 0 IRI, 1 blank node, 2 literal.
fn raw(t: &Term) -> RawTerm {
    match t {
        Term::NamedNode(n) => (0, n.as_str().to_owned(), None, None),
        Term::BlankNode(b) => (1, b.as_str().to_owned(), None, None),
        Term::Literal(l) => (2, l.value().to_owned(), Some(l.datatype().as_str().to_owned()), l.language().map(str::to_owned)),
        #[allow(unreachable_patterns)]
        _ => (2, String::new(), None, None),
    }
}

/// Raw query result: ("select", vars, rows) | ("ask", bool) | ("graph", [(s, p, o)]).
fn convert(py: Python<'_>, r: QueryResult) -> PyResult<PyObject> {
    Ok(match r {
        QueryResult::Solutions { vars, rows } => {
            let vars: Vec<String> = vars.iter().map(|v| v.as_str().to_owned()).collect();
            let rows: Vec<Vec<Option<RawTerm>>> = rows.iter().map(|r| r.iter().map(|t| t.as_ref().map(raw)).collect()).collect();
            ("select", vars, rows).into_pyobject(py)?.into_any().unbind()
        }
        QueryResult::Boolean(b) => ("ask", b).into_pyobject(py)?.into_any().unbind(),
        QueryResult::Graph(ts) => {
            let ts: Vec<(RawTerm, RawTerm, RawTerm)> = ts.iter().map(|t| (raw(&t.subject.clone().into()), raw(&t.predicate.clone().into()), raw(&t.object))).collect();
            ("graph", ts).into_pyobject(py)?.into_any().unbind()
        }
        QueryResult::Ids(_) => return Err(RdfdbError::new_err("internal: undecoded result")),
    })
}

#[pyclass(frozen, name = "Database", module = "rdfdb._rdfdb")]
struct PyDatabase { db: Arc<Database> }

#[pymethods]
impl PyDatabase {
    #[new]
    #[pyo3(signature = (path, initial=None))]
    fn new(py: Python<'_>, path: std::path::PathBuf, initial: Option<std::path::PathBuf>) -> PyResult<Self> {
        let mut opts = rdfdb::embedded::DatabaseOptions::default();
        opts.initial = initial;
        let db = py.allow_threads(|| Database::open_with(&path, opts)).map_err(io_err)?;
        Ok(PyDatabase { db: Arc::new(db) })
    }
    fn query(&self, py: Python<'_>, sparql: &str) -> PyResult<PyObject> {
        let db = Arc::clone(&self.db);
        let r = py.allow_threads(|| db.query(sparql)).map_err(query_err)?;
        convert(py, r)
    }
    fn update(&self, py: Python<'_>, sparql: &str) -> PyResult<usize> {
        let db = Arc::clone(&self.db);
        py.allow_threads(|| db.update(sparql)).map_err(query_err)
    }
    fn read(&self) -> PySnapshot { PySnapshot { snap: Arc::new(self.db.read()) } }
    fn compact(&self, py: Python<'_>) -> PyResult<()> {
        let db = Arc::clone(&self.db);
        py.allow_threads(|| db.compact()).map_err(io_err)
    }
}

#[pyclass(frozen, name = "Snapshot", module = "rdfdb._rdfdb")]
struct PySnapshot { snap: Arc<ReadSnapshot> }

#[pymethods]
impl PySnapshot {
    fn query(&self, py: Python<'_>, sparql: &str) -> PyResult<PyObject> {
        let s = Arc::clone(&self.snap);
        let r = py.allow_threads(|| s.query(sparql)).map_err(query_err)?;
        convert(py, r)
    }
}

#[pymodule]
fn _rdfdb(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyDatabase>()?;
    m.add_class::<PySnapshot>()?;
    m.add("RdfdbError", m.py().get_type::<RdfdbError>())?;
    m.add("SyntaxError", m.py().get_type::<SyntaxError>())?;
    m.add("UnsupportedError", m.py().get_type::<UnsupportedError>())?;
    m.add("StorageError", m.py().get_type::<StorageError>())?;
    Ok(())
}
