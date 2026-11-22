# TripleLight: a short technical report

TripleLight is an embedded, persistent, updatable RDF database for analytical SPARQL 1.1 on one machine, written in
Rust. You open a directory from Rust, Python, Node.js, WebAssembly, C, Go or Java, load or update triples, and run
joins and aggregations over millions to hundreds of millions of triples. The design target is a DuckDB-like shape
for RDF, not a triple-store server. This report covers the architecture, what is measured, what changed recently,
and what is still open. It links only to reports in this repository or to registered evidence; sentences marked
*unaudited* come from task measurements whose raw data is not yet in the repository.

Status: version 0.1, no published release, native store: Unix (macOS, Linux); the WebAssembly package runs in memory anywhere; Rust 1.100.

## 1. Architecture

**Crates.** `triplelight-core` (terms, parsers, numbers), `triplelight-iri` (IRI validation and resolution),
`triplelight-sparql` (SPARQL parser and algebra), `triplelight-plan` (statistics, join ordering),
`triplelight-exec` (vectorized operators) and `triplelight-storage` (dictionary, permutations, WAL, snapshots).

**Terms and IDs.** Every term is a 64-bit ID whose top four bits tag its kind. Some kinds carry their value inline in the low 60 bits
with no dictionary entry (see `model.rs`); every other term is a dictionary entry. Operators work on IDs and decode a term only when a result, a string function or a
filter needs its text.

**Storage.** A snapshot holds six sorted permutations of the triples (SPO, SOP, PSO, POS, OSP, OPS) as compressed,
checksummed blocks read through a shared, bounded block cache, plus a front-coded dictionary. Updates go to a
write-ahead log with group commit and fsync before acknowledgement, are overlaid on the base as a delta, and are
merged into a new base by compaction. Queries read a consistent snapshot. See
[persistence.md](../persistence.md).

**Query engine.** SPARQL text is parsed to an algebra, planned with cost-based join ordering from statistics
(characteristic sets and predicate counts, persisted beside the base so a store is ready in under a second), and
executed with worst-case-optimal leapfrog joins, bind joins, filter pushdown, and hash aggregation and top-k that
work on IDs. A feature the engine does not implement is rejected with an "unsupported" error rather than answered
partially.

**Correctness gate.** Every benchmark answer is reduced to a canonical multiset and compared with Apache Jena.
CI requires the W3C SPARQL 1.1 query suite to stay at 323 passing tests ([floor](../../ci/w3c_floor.json)).

## 2. What is measured

All numbers are in [benchmarks.md](../benchmarks.md) and [evaluation.md](evaluation.md), with method and raw data.
Systems are always compared in the same session on the same host, and server and in-process numbers are never
mixed.

- **Small and medium scale (10–15M triples).** On LUBM-100, WatDiv-100 and BSBM-10M, TripleLight is the fastest
  system measured, in memory and on disk, against Jena TDB2, RDF4J, Oxigraph, Virtuoso 7 and QLever
  (evaluation.md, rounds 5 and 8).
- **Large scale (100–140M triples).** On LUBM-1000 and BSBM-100M, with every system at about 10 GB of cache,
  TripleLight's disk store has the lowest total on LUBM-1000 (6.4 s, against 30.5 s for QLever; Virtuoso's total is
  understated because its endpoint truncates two answers) and is level with Virtuoso on BSBM-100M (evaluation.md,
  rounds 11 and 12).
- **Analytical, in process.** Through its own Python API on an on-disk store, TripleLight finishes the BSBM BI
  workload at 10M triples in 11.6 / 13.1 s (two passes), against 130.0 / 110.6 s for DuckDB running hand-written
  SQL over one triples table. On the LUBM-100 and WatDiv aggregate suites it is 0.67 s and 0.20 s against 41.7 s and
  27.1 s or more for the others. All requests passed the frozen answer gates against Jena.
  [Evidence](https://app.raft.build/artifacts/Azoz2CgbhGKQtHczyXUoiY). DuckDB has no SPARQL; it is a SQL baseline,
  and a better SQL layout was not measured.
- **Startup.** With persisted statistics, an embedded disk store has planner statistics 0.74–0.91 s after open
  starts, against 20.5–25.8 s with a rebuild scan ([report](../startup36-persisted-stats/README.md)).

These are descriptive, single-host measurements with two passes; they are not significance tests.

## 3. Recent work

**Named graphs (#36).** The embedded database, the persistent store, the bindings (including WebAssembly) and the
SPARQL layer now support `GRAPH`, `FROM`, `FROM NAMED`, updates into named graphs, graph management
(`CREATE`, `DROP`, `CLEAR`, `ADD`, `MOVE`, `COPY`) and N-Quads and TriG loading. A disk base can hold named graphs
(`RDFSNAP5`): the default graph as before, six graph-first quad orders and a checksummed catalog of graphs. A
dataset without named graphs writes exactly the `RDFSNAP4` base. Compaction moves graphs from the WAL into the
base; a model test runs 60 rounds of random updates, compactions and reopens against a set model. Still different from the full
standard: `LOAD`, blank-node graph names and empty TriG graph blocks are not supported (section 4).

**Own parsers (#45).** TripleLight no longer depends on Oxigraph's parsers at run time. Its own IRI resolver
(`triplelight-iri`), N-Triples/N-Quads parser and Turtle/TriG parser accept and reject exactly what the previous
`oxiri`/`oxttl` code did and emit the same statements. Parity is checked by differential tests against those crates:
the W3C RDF 1.1 N-Triples, N-Quads, Turtle and TriG suites (161 and 992 files), curated edge cases, and
hundreds of thousands of random documents, plus every statement of LUBM-100, BSBM-10M and WatDiv-100. Stores built
with the old and new N-Triples parser are byte-identical. On BSBM-10M, N-Triples parsing takes 8.4–9.2 s against
10.1–10.5 s for the old parser, and Turtle is at parity (11.6–11.9 s against 11.4–11.9 s). The old crates remain as test-only comparators, and
`THIRD_PARTY_NOTICES.md` records the MIT notices for the code the parsers are derived from.

**Disk build and dictionary layout (#54, #55).** Profiling the external build of BSBM-100M showed two near-raw-size
copies of the dictionary, one ordered by term and one by ID, making up about three quarters of the store. IDs inside a
kind already follow term-text order, so one front-coded file in ID order, with each block's last term kept in the
directory, serves both lookups (`RDFSNAP4`). On BSBM-100M the store shrank from 12.24 GiB to 7.75 GiB (−37%), the dictionary conversion phase ran about 58 s
(30%) faster, and `verify` passes ([evidence](../unified-dictionary-100m-v1/README.md)). *Unaudited:* an
Arrow-decode A/B showed no regression. Earlier fixes freed scratch artifacts as soon as they were consumed and streamed the final
term merge into the dictionary pass; peak scratch disk during the build fell from about 60 GB to about 42–44 GB.
Older snapshot formats stay readable; there is no downgrade.

**Hardening.** A term whose N-Triples text exceeds 16 MiB is now refused at commit instead of being acknowledged
and then blocking every compaction (#95). `verify` is 3.4–3.7× faster and still exact (#71).

## 4. Open limits

- **Build speed and footprint.** Building the disk store is TripleLight's widest gap to QLever: LUBM-1000 took
  1,035 s against 425 s (benchmarks.md, round 9), and BSBM-100M's store was 12.3 GB against 3.8 GB for QLever before the
  dictionary change (QLever's size is *unaudited*; the change above narrows the size gap but the time gap is not re-measured against
  QLever). The build is mostly single-threaded, and the build needs tens of gigabytes of scratch disk.
- **Memory.** QLever uses about a tenth of TripleLight's memory on BI 100M (0.4–0.5 GiB against 3.7–4.1 GiB), and is
  faster on BI q3.
- **Small builds.** An in-memory build still writes the older two-file dictionary (`RDFSNAP3`). Renumbering IDs to
  write `RDFSNAP4` directly costs 1.3–2× build time at 10M and was not done (*unaudited*).
- **SPARQL coverage.** `LOAD`, `SERVICE` and `BNODE(string)` are unsupported, as are blank-node graph names in
  N-Quads and TriG. Numeric arithmetic is exact only within 96 bits for decimals and 64 bits for integers; results
  outside are errors, and decimal underflow is stricter than XPath.
- **Operation.** Results are materialized before output; one process per store directory; no licence has been chosen
  yet, so the code is not licensed for reuse.
- **Evidence.** Most large-scale numbers come from one host and one or two passes. A virtual-machine session can
  drift 2–3× from another, so only same-session numbers are compared.

## 5. Where to go next

The measured gaps point to three jobs: a parallel, lower-scratch disk build; a smaller resident footprint for the
BI templates where QLever wins; and moving the remaining Oxigraph-derived term types onto TripleLight's own
(step 4 of #45).
