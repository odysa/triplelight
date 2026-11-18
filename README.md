# rdfdb

rdfdb is an RDF database and SPARQL 1.1 query engine written in Rust. It is aimed at analytical workloads, in the
way DuckDB is for tables. It runs as an embedded library (Rust and Python) or as a SPARQL HTTP server, and keeps
data in memory or in a persistent, checksummed on-disk store with a write-ahead log.

Main design points: dictionary-encoded 64-bit term IDs with inline small values, six sorted triple permutations,
worst-case-optimal (leapfrog) and bind joins chosen by a cost-based planner with statistics, filter pushdown and
compiled numeric filters, hash aggregation and top-k on term IDs, and a sharded block cache over compressed
blocks. See [DESIGN.md](DESIGN.md) and [docs/](docs/).

## Benchmark results

All comparisons run every system on the same machine in the same session, with each system's answers checked
against Apache Jena. Numbers are sums of per-query medians unless stated. Lower is better.

### Analytical: BSBM Business Intelligence, 10M triples (two-engine rerun)

Same Linux VM (4 CPUs, 12 GiB cap per engine), two passes in reversed order. Each number is the sum of 45
measured request times after 15 warmups. rdfdb on main `20b257a`; DuckDB 2.0.0 over one lossless triples table
with hand-written SQL per query template. All 240 requests return complete, correct answers for both engines.
This is a bounded same-session comparison of two engines, not the official BSBM throughput procedure.

| | Pass 1 (s) | Pass 2 (s) |
|---|---:|---:|
| **rdfdb** | **14.5** | **14.1** |
| DuckDB 2.0.0 | 114.0 | 112.9 |

rdfdb is faster on all 8 query templates in both passes (DuckDB takes 7.9× / 8.0× as long). Timings drift between
VM sessions: in the previous session, with rdfdb at `738f0cb`, the totals were 27.2 / 28.5 s against DuckDB's
55.3 / 64.5 s, so compare engines only within a session. In an earlier pass of this benchmark, neither QLever nor Virtuoso had a qualified total. QLever matched
Jena's answers on 9 of 60 requests under strict checking; all of its other mismatches are result datatypes
(computed float aggregates, and `xsd:int` instead of `xsd:integer` for COUNT, with which mapped it passes 21).
Virtuoso passed 29 of 60. Its q4 and q6 failures are datatypes, but all 12 q5 instances return wrong result
sets (for example zero rows where Jena returns 14), and its reverse-order pass hit the client timeout. Evidence: [matched-vm-round3-v1](https://github.com/odysa/rdfdb/tree/5ff7d67eeb77ccdc348e5344fb571cf262d23a04/docs/analytical-benchmark/matched-vm-round3-v1);
previous session in [matched-vm-round2-v1](https://github.com/odysa/rdfdb/tree/67f1fe7da321c67924b54c62a0be0299dba06f15/docs/analytical-benchmark/matched-vm-round2-v1),
first pass with all four engines in [matched-vm-v1](https://github.com/odysa/rdfdb/tree/67f1fe7da321c67924b54c62a0be0299dba06f15/docs/analytical-benchmark/matched-vm-v1).

### Large scale: LUBM-1000 (133.6M triples) and BSBM-100M (98.9M triples)

Round 13, one Docker Linux VM session (arm64, 16 GB RAM, 10 CPUs). rdfdb runs from its disk store with a 10 GB
cache; Virtuoso 7 has a 10.6 GB buffer pool; QLever runs with its result cache disabled. Times include HTTP
transfer of JSON results. Total ms / geometric mean ms per query:

| | rdfdb (disk) | QLever | Virtuoso 7 |
|---|---:|---:|---:|
| LUBM-1000, 14 queries | **6,747 / 29.0** | 42,474 / 49.1 | 31,560* / 67.1 |
| LUBM-1000 without Q6, Q14 | **726 / 13.4** | 1,903 / 18.0 | 8,471 / 28.5 |
| BSBM-100M, 20 queries | 279 / **8.6** | 1,151 / 37.9 | **240** / 9.5 |

\* Virtuoso's HTTP endpoint stops at 1,048,576 rows, so its Q6 and Q14 answers (7.9M rows each) are incomplete.

On LUBM-1000 rdfdb is the fastest system on 9 of 14 queries in round 13. In round 14, after the hash aggregation
change, it beat QLever on all 12 queries QLever completed (for example Q2 42.6 vs 84.0 ms, Q9 497 vs 2,561 ms;
QLever's Q6 and Q14 responses failed). On BSBM-100M rdfdb and Virtuoso are close: rdfdb is fastest on 15 of 20
queries, Virtuoso has the lower total. Raw results: [bench/results/r13-lubm1000](bench/results/r13-lubm1000),
[r13-bsbm100](bench/results/r13-bsbm100), [r14-lubm1000](bench/results/r14-lubm1000).

### 10–15M triples, all systems

Round 8, one VM session (QLever and Virtuoso only ship Linux builds). Total ms / geometric mean ms:

| | rdfdb memory | rdfdb disk | Virtuoso 7 | QLever |
|---|---:|---:|---:|---:|
| LUBM-100 | **289 / 6.8** | 378 / 8.0 | 9,441 / 23.1 | 2,103 / 15.2 |
| WatDiv-100 | **151 / 4.1** | 179 / 5.0 | 1,727 / 6.3 | 1,157 / 16.0 |
| BSBM-10M | **95** / 4.4 | 96 / **4.2** | 98 / 4.8 | 500 / 16.3 |

Round 5, macOS host, systems that run natively:

| | rdfdb memory | rdfdb disk | RDF4J memory | RDF4J native | Jena TDB2 | Oxigraph |
|---|---:|---:|---:|---:|---:|---:|
| LUBM-100 | **310 / 1.7** | 420 / 2.2 | 1,014 / 7.2 | 3,632 / 13.4 | 7,798 / 25.9 | 71,521 / 55.6 |
| WatDiv-100 | **119 / 0.8** | 141 / 1.3 | 301 / 2.8 | 879 / 5.4 | 10,198 / 9.0 | 161,338 / 57.2 |
| BSBM-10M | **27 / 0.6** | 47 / 0.9 | 113 / 2.8 | 203 / 3.1 | 1,998 / 7.7 | 1,040 / 4.3 |

### Embedded, in-process: rdfdb vs LadybugDB

Both through their Python APIs in one process, every result row decoded to RDF terms, no HTTP. Sum of per-query
medians (ms):

| | rdfdb (Python objects) | Ladybug, generic RDF mapping | Ladybug, LUBM-tuned schema |
|---|---:|---:|---:|
| LUBM-100, 14 queries | **2,835** | 14,757 | 4,484 |
| BSBM-10M, 20 queries | **27** | 4,009 | not built |

LUBM Q6 and Q14 return 796K rows each and dominate these totals; without them rdfdb takes 86 ms and tuned Ladybug
154 ms. On BSBM only Ladybug's generic mapping was measured, so the gap there reflects schema as
well as engine. Ladybug opens a store faster (30–240 ms against rdfdb's 3.1–4.3 s until statistics are ready).
Details: [docs/embedded/README.md](docs/embedded/README.md).

### Where rdfdb is behind

- Loading: building the LUBM-1000 disk store took 1,035 s against QLever's 425 s in round 9; the parallel loader
  merged since then is about 30% faster but has not been re-measured at that scale.
- Store open: statistics take several seconds to become ready after opening a large store.

Full method and more results: [docs/report/evaluation.md](docs/report/evaluation.md) and
[bench/README.md](bench/README.md).

## Usage

```sh
cargo build --release
# In-memory server over a file
./target/release/rdfdb serve data.nt 7878
# Persistent store, created from a file on first start
./target/release/rdfdb serve-store ./store 7878 --init data.nt
# One-off query
./target/release/rdfdb query data.nt 'SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }'
```

The server speaks the SPARQL 1.1 protocol at `http://127.0.0.1:7878/sparql`. For embedding, see the Rust API in
[docs/embedded/README.md](docs/embedded/README.md) and the Python package in [bindings/python](bindings/python).
