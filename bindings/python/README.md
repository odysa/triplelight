# TripleLight for Python

The embedded TripleLight RDF database in a Python process: SPARQL 1.1 queries and updates on a local directory, no
server. Needs Python 3.9+ and Rust 1.100 to build from source; Linux or macOS.

```sh
python3 -m venv .venv && source .venv/bin/activate    # `maturin develop` needs an active virtualenv
pip install maturin
cd bindings/python && maturin develop --release
```

```python
import triplelight

db = triplelight.Database("my.db")        # opens or creates a store in this directory
db.update('INSERT DATA { <urn:alice> <urn:age> 30 . <urn:bob> <urn:age> 41 }')
for row in db.query("SELECT ?who ?age WHERE { ?who <urn:age> ?age } ORDER BY ?age"):
    print(row["who"], row["age"])         # urn:alice 30, then urn:bob 41
```

- `Database(path, initial=None)` opens a store (`initial` is an N-Triples file loaded only into a new directory);
  `Database.in_memory()` makes a database that is not persisted. One process may open a directory at a time
  (`LockedError` otherwise).
- `update` is atomic and durable (it fsyncs before returning) and returns the number of changed triples. Updates can
  name graphs (`GRAPH`, `WITH`, `CREATE`, `DROP`, ...).
- `load(path)` imports a `.nt`, `.ttl`, `.nq` or `.trig` file atomically; quads go to their named graphs.
- `query` returns a `Result`: iterate it for SELECT rows (a row is a tuple that also takes `row["var"]`), `bool(result)`
  for ASK, `.triples` for CONSTRUCT. IRIs come back as `IRI` and blank nodes as `BNode`. A literal is a Python `str` (plain strings), an `int` (canonical
  `xsd:integer` that fits a signed 64-bit integer) or a `bool` (`xsd:boolean` written `true` or `false`); any other
  literal is `Literal(value, datatype, lang)`. An unbound variable is `None`.
- `query_json` answers SELECT and ASK and returns SPARQL 1.1 Query Results JSON as a string. It serializes straight
  from the store without creating Python row objects, which suits large results.
- `with db.read() as snap:` gives a consistent snapshot for several queries; later updates do not change it.
- `compact()` merges updates into a new base. `stats_ready()` is true once background optimizer statistics are built
  (queries work before that).
- `query` and `query_json` take `timeout=` (seconds) and `memory_mb=`: a query past either raises `QueryTimeout` or
  `ResourceLimitError`, never a partial result. With `spill=True`, a GROUP BY over `memory_mb` spills its input to
  `TRIPLELIGHT_SCRATCH_DIR` (else the system temporary directory) and finishes instead; by default nothing is written. On Unix, Ctrl-C cancels a query running on the main thread and raises
  `KeyboardInterrupt` (the SIGINT action is installed on the first such query and chained to Python's handler;
  `triplelight.set_cancel_on_ctrl_c(False)` turns it off). On Windows, Ctrl-C is raised once the query ends.
- Errors are `TripleLightError` subclasses: `SyntaxError`, `UnsupportedError`, `StorageError`, `LockedError`,
  `GraphStateError`, `QueryTimeout`, `QueryCancelled` and `ResourceLimitError`.

Test: `pytest -q tests` in this directory, after `maturin develop`.
