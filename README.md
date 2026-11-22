# TripleLight

**An embedded RDF database for analytical SPARQL queries, written in Rust.**

TripleLight runs in your application over an in-memory graph or a durable local store. Use it for
`GROUP BY`, `DISTINCT`, and joins over RDF data. An optional HTTP server exposes the SPARQL protocol.

> **Early development (0.1).** Build from source on Linux or macOS with Rust 1.100.0, as pinned in
> [rust-toolchain.toml](rust-toolchain.toml). APIs and the on-disk format may change.

[Quickstart](#quickstart) · [Language bindings](#embedded-apis) · [Benchmarks](#performance) · [Documentation](#learn-more)

## Quickstart

This example loads one N-Triples statement and returns `true` in the SPARQL JSON result.

```sh
git clone https://github.com/odysa/triplelight.git
cd triplelight
printf '%s\n' '<urn:alice> <urn:knows> <urn:bob> .' > example.nt
cargo run --release -- query example.nt 'ASK { <urn:alice> <urn:knows> <urn:bob> }'
```

### Embedded APIs

| Language | Start here |
| --- | --- |
| Rust | [Embedded API, persistent stores and bulk loading](docs/embedded/README.md) · [async API on any executor](docs/embedded/README.md#async) |
| Python | [Install from source and query a graph](#python) |
| JavaScript / TypeScript | [Node.js binding](bindings/node/README.md) · [WebAssembly for browsers and other runtimes](bindings/wasm/README.md) |
| C, Go and Java | [C ABI](bindings/c/README.md) · [Go binding](bindings/go/README.md) · [Java binding](bindings/java/README.md) |

### Python

From the repository root, build the binding in a virtual environment.

```sh
python3 -m venv .venv
source .venv/bin/activate
python -m pip install maturin
(cd bindings/python && maturin develop --release)
```

```python
from triplelight import Database

db = Database.in_memory()
db.update("INSERT DATA { <urn:alice> <urn:knows> <urn:bob> }")
for row in db.query("SELECT ?who WHERE { ?who <urn:knows> <urn:bob> }"):
    print(row["who"])
```

An in-memory database is discarded when its last handle is released. Open `Database("example.db")` for
durable local storage. Updates are atomic, persistent updates sync the write-ahead log before returning, and
`db.read()` gives a consistent snapshot across queries. See the [Python binding source and tests](bindings/python).

### Optional SPARQL HTTP server

Enable the `server` Cargo feature to serve the quickstart dataset over HTTP.

```sh
cargo build --release --features server

# Serve a file over the SPARQL 1.1 protocol at http://127.0.0.1:7878/sparql (read-only)
./target/release/triplelight serve example.nt 7878

# Persistent store, created from example.nt on first start; SPARQL Update at /update with a token
TRIPLELIGHT_UPDATE_TOKEN=change-me ./target/release/triplelight serve-store ./store 7878 --disk --init example.nt
```

The server listens on 127.0.0.1 unless `TRIPLELIGHT_HOST` is set. `serve-store` accepts updates only when
`TRIPLELIGHT_UPDATE_TOKEN` is set (clients then send `Authorization: Bearer <token>`); otherwise `/update` is
disabled. For local scripts you can pass `--allow-unauthenticated-updates` instead; it works only on 127.0.0.1 and
still refuses requests from browser pages. The token travels in plain HTTP, so on an untrusted network put the server behind a
TLS-terminating proxy.

Query the server with any SPARQL client, for example:

```sh
curl -s http://127.0.0.1:7878/sparql --data-urlencode 'query=SELECT * WHERE { ?s ?p ?o } LIMIT 5'
```

## Performance

- [Benchmark overview](docs/benchmark-overview.md) gives the embedded Python, Node.js, and JVM results and the HTTP charts.
- [Benchmark methodology and full results](docs/benchmarks.md) links the query sets, answer checks, timings, and raw evidence.
- [Embedded-store inventory](docs/embedded-store-inventory.md) tracks which engines and storage modes have been evaluated.

The embedded tests time full result consumption through each language API. The HTTP tests time the request through
receipt of the complete response. The reports give the workload, store mode, resource limits, and Apache Jena answer
checks for each comparison.

## Features

- **SPARQL 1.1 query and update**: SELECT, ASK, CONSTRUCT, OPTIONAL, UNION, MINUS, FILTER, EXISTS, subqueries,
  VALUES, BIND, GROUP BY with all standard aggregates, ORDER BY, LIMIT/OFFSET, `INSERT DATA`, `DELETE DATA`,
  `DELETE/INSERT WHERE`.
- **Named graphs** (Rust, Python, Node.js, WebAssembly and the C, Go and Java bindings): `GRAPH`, `FROM` and `FROM NAMED` in
  queries; updates into named graphs (`GRAPH` in `INSERT`/`DELETE`, `WITH`, `USING`); and graph management with
  `CREATE`, `DROP`, `CLEAR`, `ADD`, `MOVE` and `COPY`. A request's operations apply in order and commit together.
  `load` also reads N-Quads (`.nq`) and TriG (`.trig`) files into their named graphs. Persistent stores record
  named-graph changes in the write-ahead log and compact named graphs into the base.
- **Exact numbers, within a range**: `xsd:decimal` arithmetic is exact within 96 bits and 28 fractional digits, and
  `xsd:integer` within 64 bits; results outside that range are query errors (Jena uses arbitrary precision).
  Non-terminating decimal quotients such as 1/3 are rounded to 28 digits. Decimal underflow is an error, which is
  stricter than XPath (XPath returns 0.0), so numeric behavior is not fully SPARQL-conformant.
- **Storage**: dictionary-encoded 64-bit term IDs, four sorted triple orders that also answer the other two (all six
  with `--orders=6`), compressed and checksummed blocks
  with a shared cache, write-ahead log with group commit, snapshot reads, and compaction.
- **Query engine**: cost-based join ordering from statistics, worst-case-optimal (leapfrog) and bind joins,
  filter pushdown, hash aggregation and top-k on term IDs.
- **W3C SPARQL 1.1 test suite**: CI requires at least 323 passing tests
  ([floor](ci/w3c_floor.json), [older detailed runs](bench/results/w3c-sparql11)).

### Other supported features

Property paths; `DESCRIBE`; the hash functions, `NOW`, `RAND`, `UUID`, `STRUUID` and `BNODE()`; Arrow C stream
output from the C, Go and Java bindings; persisted optimizer statistics (in the
[startup36 session](docs/startup36-persisted-stats/README.md), statistics were ready 0.74–0.91 s from the start of open
with a saved statistics file, versus 20.5–25.8 s with a rebuild scan); and an optional HTTP server whose updates are
off by default and can be enabled with a token or, on loopback only, without one.

### Current limits

- The query and update surface excludes `LOAD`, `SERVICE` and `BNODE(string)`.
- N-Quads and TriG imports require IRI graph names. Empty TriG blocks create no graph.
- Results are materialized before output; Arrow output then streams that result in batches.
- One process per store directory (enforced by a file lock).

## Learn more

- [Short technical report](docs/report/technical-report.md) covers the architecture, measured results, and current limits.
- [Architecture](DESIGN.md) and [design rationale](docs/design/first-principles.md)
- [Persistence and disk formats](docs/persistence.md)
- [Query optimizer](docs/optimizer.md)
- [Documentation](docs/)

## License

TripleLight is licensed under the [MIT License](LICENSE). See [third-party notices](THIRD_PARTY_NOTICES.md) for incorporated code.
