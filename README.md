# TripleLight

**An embedded, analytical RDF database with SPARQL 1.1, written in Rust.**

TripleLight is aimed at RDF analytics the way DuckDB is aimed at tables: aggregates, GROUP BY, DISTINCT and
large joins over millions to hundreds of millions of triples. You can use it as:

- a **library** inside your Rust or Python program, with no server to run, or
- a **SPARQL 1.1 HTTP server**, an optional feature for tools that need one.

Data lives in memory or in a persistent on-disk store with a write-ahead log. Benchmark answers are checked
against Apache Jena, and SPARQL features TripleLight does not implement are rejected with an "unsupported" error
instead of returning a partial result.

> **Status: early (0.1).** There are no published releases yet; build from source. The on-disk format and APIs may still
> change. Unix only (Linux, macOS), Rust 1.89+.

## Quickstart

### Command line

```sh
git clone https://github.com/odysa/triplelight.git triplelight && cd triplelight
cargo build --release            # binary: ./target/release/triplelight

# Ask a question of an N-Triples or Turtle file
./target/release/triplelight query data.nt 'SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }'

```

### Optional: SPARQL HTTP server

TripleLight is an embedded library first. The HTTP server is an optional Cargo feature:

```sh
cargo build --release --features server

# Serve a file over the SPARQL 1.1 protocol at http://127.0.0.1:7878/sparql (read-only)
./target/release/triplelight serve data.nt 7878

# Persistent store, created from data.nt on first start; SPARQL Update at /update with a token
TRIPLELIGHT_UPDATE_TOKEN=change-me ./target/release/triplelight serve-store ./store 7878 --disk --init data.nt
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

### Python

```sh
python3 -m venv .venv && source .venv/bin/activate    # maturin develop needs an active virtualenv
pip install maturin
cd bindings/python && maturin develop --release
```

```python
import triplelight

db = triplelight.Database("my.db")                  # opens or creates a store in this directory
db.update('INSERT DATA { <urn:alice> <urn:age> 30 . <urn:bob> <urn:age> 41 }')
db.load("more-data.nt")                             # bulk import N-Triples or Turtle

for row in db.query("SELECT ?who ?age WHERE { ?who <urn:age> ?age } ORDER BY ?age"):
    print(row["who"], row["age"])                   # IRIs, ints, strings come back as Python values
```

Updates are atomic and durable (fsync before `update` returns). `db.read()` gives a consistent snapshot for several
queries. See [bindings/python](bindings/python).

### Rust

```toml
[dependencies]
triplelight = { git = "https://github.com/odysa/triplelight" }
```

```rust
use triplelight::embedded::Database;
use triplelight::exec::QueryResult;

let db = Database::open("./example.db")?;
db.update("INSERT DATA { <https://ex/s> <https://ex/p> <https://ex/o> }")?;
match db.read().query("ASK { <https://ex/s> <https://ex/p> <https://ex/o> }")? {
    QueryResult::Boolean(found) => assert!(found),
    _ => unreachable!(),
}
```

More in [docs/embedded/README.md](docs/embedded/README.md), including bulk loading of large files with
`triplelight build-disk`.

## Performance

Benchmarks compare like with like: **embedded engines in the same process** (no network on any side), and
**servers over HTTP**. The two kinds of numbers are never mixed in one table.

### Embedded (in-process): Python API

Each engine runs inside one Python process through its own API and its own on-disk store, in a container capped at
4 CPUs and 12 GiB. Times are summed query seconds per pass, from submitting the query until the full result has been
converted to Python values. Two passes (pass 2 in reversed order). BI: 15 warm-up then 45 timed requests per pass;
each aggregate phase: one complete warm pass, then one timed pass. **Lower is better.**

| Workload | TripleLight | DuckDB (SQL baseline) | pyoxigraph |
|---|---:|---:|---:|
| BSBM BI, 10M triples (45 timed requests) | **11.6 / 13.1 s** | 130.0 / 110.6 s | not ranked: qualification incomplete |
| LUBM-100 aggregates (30 queries) | **0.67 / 0.64 s** | 50.4 / 40.0 s | 41.7 / 34.5 s |
| WatDiv aggregates (42 queries) | **0.20 / 0.23 s** | 27.1 / 18.9 s | 71.6 / 71.8 s |

All 1,104 scheduled requests passed the frozen answer gates against Apache Jena, with no retries or out-of-memory
failures. The gates are: BI value-compatible (registered numeric tolerance, exact anchors, ORDER BY rules), and for
the aggregates COUNT-compatible with exact group terms and multiplicity. They are not exact typed-result equality on
every query. TripleLight ran at commit `e2e9cb2f`. Engine cache settings differ between engines. DuckDB has no SPARQL;
it answers SQL over one triples table: hand-written templates for BSBM BI, and a translator written only for the
aggregate suite for LUBM and WatDiv (generation inside the timer). pyoxigraph has no BI total because its BI
qualification timed out before completing (the 11 answers it produced passed); separately, in a nine-term identity
test it merges literals that RDF treats as distinct terms. Evidence:
[report and raw answers]((internal link removed)). A Node.js in-process comparison
(TripleLight, oxigraph, quadstore + Comunica) is in progress.

### Server (HTTP)

Two analytical workloads, each measured in its own session on one Linux VM, with every engine capped at 4 CPUs and
12 GiB. They were not measured together on one build: BI used TripleLight commit `ef7a778` and the aggregate suite
`9a2f49b`.
Left: total query time in **seconds, log scale**. Right: peak container memory in **GiB**. **Lower is better** in
both. Each engine ran two passes (pass 2 in reversed order); both are shown because timings drift between passes.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/bench-bi100m-dark.svg">
  <img alt="BSBM Business Intelligence, 98.9M triples. Total time, pass 1 / pass 2: TripleLight 11.74 / 6.73 s, QLever 24.18 / 24.51 s, DuckDB (hand-written SQL) 573.8 / 443.5 s. Peak memory: TripleLight 3.7 / 4.1 GiB, QLever 0.51 / 0.39 GiB, DuckDB 11.4 / 12.0 GiB." src="docs/images/bench-bi100m-light.svg">
</picture>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/bench-aggregate72-dark.svg">
  <img alt="72 aggregate queries on LUBM-100 and WatDiv. Total time, pass 1 / pass 2: TripleLight 0.58 / 0.54 s, Virtuoso 4.61 / 4.50 s, QLever 5.11 / 4.60 s, DuckDB (SQL from a suite-specific translator) 73.1 / 73.6 s. Peak memory: TripleLight 0.97 / 0.94 GiB, Virtuoso 10.4 GiB, QLever 0.29 GiB, DuckDB 3.4 GiB." src="docs/images/bench-aggregate72-light.svg">
</picture>

| Workload | What is timed | Answers checked |
|---|---|---|
| **BSBM BI, 98.9M triples** | 45 requests per pass over 8 query templates, end-to-end HTTP | Against Jena; all engines pass a value-only check, TripleLight and DuckDB also pass the datatype check on 60/60 |
| **Aggregate72** (LUBM-100 13.4M + WatDiv 10.9M triples) | 72 COUNT / GROUP BY / DISTINCT queries per pass, end-to-end HTTP | Against Jena; all engines pass with COUNT `xsd:int` accepted as `xsd:integer` |

**What this shows, honestly:**

- TripleLight is the fastest of these engines on both workloads in total time: about 2–4× QLever on BI at 98.9M
  triples, and about 8–9× QLever and Virtuoso on the aggregate suite.
- **QLever uses much less memory** (about a tenth of TripleLight's on BI 100M) and is faster on one BI query
  template (q3). Memory and q3 are our main open gaps.
- On WatDiv alone, TripleLight and Virtuoso are close (0.26 / 0.23 s vs 0.30 / 0.31 s).
- **DuckDB does not support RDF or SPARQL**, so it is a SQL baseline here, not an RDF engine. The triples are loaded
  into one DuckDB table (subject, predicate, and object with its datatype and language). For BSBM BI, each request
  fills a hand-written SQL template for its query template; for the aggregate suite, a small translator written for
  that suite turns each SPARQL COUNT query into SQL. In both cases the SQL generation runs inside the timed request.
  A better SQL layout (integer IDs, one table per predicate) would likely be much faster and has not been measured.
- Two passes on a shared machine is a bounded observation, not a significance test.

Full tables, per-query breakdowns, more datasets (LUBM-1000, BSBM, WatDiv), other engines (Jena, RDF4J,
Oxigraph, Virtuoso, LadybugDB), methodology and raw evidence: **[docs/benchmarks.md](docs/benchmarks.md)**.
The charts are generated by [bench/charts.py](bench/charts.py) from the published evidence.

## Features

- **SPARQL 1.1 query and update**: SELECT, ASK, CONSTRUCT, OPTIONAL, UNION, MINUS, FILTER, EXISTS, subqueries,
  VALUES, BIND, GROUP BY with all standard aggregates, ORDER BY, LIMIT/OFFSET, `INSERT DATA`, `DELETE DATA`,
  `DELETE/INSERT WHERE`.
- **Exact numbers, within a range**: `xsd:decimal` arithmetic is exact within 96 bits and 28 fractional digits, and
  `xsd:integer` within 64 bits; results outside that range are query errors (Jena uses arbitrary precision).
  Non-terminating decimal quotients such as 1/3 are rounded to 28 digits. Decimal underflow is an error, which is
  stricter than XPath (XPath returns 0.0), so numeric behavior is not fully SPARQL-conformant.
- **Storage**: dictionary-encoded 64-bit term IDs, six sorted triple orders, compressed and checksummed blocks
  with a shared cache, write-ahead log with group commit, snapshot reads, and compaction.
- **Query engine**: cost-based join ordering from statistics, worst-case-optimal (leapfrog) and bind joins,
  filter pushdown, hash aggregation and top-k on term IDs.
- **W3C SPARQL 1.1 test suite**: 281 pass, 6 fail, 44 unsupported ([details](bench/results/w3c-sparql11)).

### Not supported yet

- Property paths, named graphs (`GRAPH`, `FROM`, N-Quads/TriG), `SERVICE`, `DESCRIBE`, and some functions (hashes, `NOW`,
  `UUID`, …). These return an "unsupported" error.
- Results are fully materialized in memory; there is no streaming or Arrow output yet.
- One process per store directory (enforced by a file lock).
- Opening a large store takes seconds until optimizer statistics are ready (42.8 s for a 100M-triple store);
  queries work before that with simpler plans.

## Learn more

- [DESIGN.md](DESIGN.md) and [docs/design/first-principles.md](docs/design/first-principles.md): architecture and
  design rationale
- [docs/benchmarks.md](docs/benchmarks.md): all benchmark results and how to read them
- [docs/embedded/README.md](docs/embedded/README.md): Rust API and bulk loading
- [docs/](docs/): optimizer, storage and benchmark reports

## License

No license has been chosen yet. Until one is added, the code is not licensed for reuse.
