# Paper evaluation plan (task #92), draft

Goal: every number in a research paper comes from one dedicated Linux machine, measured under a protocol registered
in advance, reproducible from an archived artifact. Scripts and answer checks are qualified on the laptop first; the
machine is rented only for the measurement window.

## Claims the evaluation must support (draft)
- C1 Embedded analytical SPARQL: in-process analytical queries (joins + aggregation on term IDs) are faster than
  embedded RDF stores and a triples-table SQL engine, and competitive with the fastest servers.
- C2 Four index orders suffice: SPO, PSO, POS, OSP serve all benchmark queries; SOP/OPS are derived exactly; stores
  16–36% smaller with unchanged query time.
- C3 Bulk build: fixed-width sorting (#90) and duplicate-free term sorting (#91) reduce build time and scratch.
- C4 (if supported) Robust planning: characteristic-set planning + WCOJ choose plans within X of the best forced order.

## Systems (native on the same machine, versions pinned)
Server (HTTP, SPARQL protocol): TripleLight (disk, server feature), QLever, Virtuoso 7, Jena TDB2/Fuseki, Oxigraph,
RDF4J (native and LMDB). Embedded (language API): TripleLight Python/Java/Rust, pyoxigraph, rdflib backends that
qualified, Blazegraph (Java), DuckDB (SQL baseline: single table and six sorted copies, #72).
Each system: documented configuration, memory budget matched where it has one, index layout recorded (#72 inventory).

## Datasets and scales
LUBM-1000 (134M) and LUBM-8000 (~1.07B); BSBM-100M and BSBM-1B (BI templates at scale-matched constants — the 10M
constants used so far under-exercise 100M); WatDiv-100M and WatDiv-1B. Optional real data: a Wikidata truthy subset.

## Workloads
Benchmark query suites (LUBM 14, WatDiv 20 templates × instances, BSBM explore 20), analytical (BSBM BI, LUBM/WatDiv
aggregate suites), large-result queries (LUBM Q6/Q14), bulk build, open-to-ready, update throughput (WAL commit).

## Protocol
- One system at a time; machine otherwise idle; CPU governor and frequency recorded.
- Cold: drop the OS page cache before each process (`echo 3 > /proc/sys/vm/drop_caches`), fresh process.
  Warm: one warm-up pass, then 5 measured runs. Report median and min–max (or IQR) per query; geomean per suite.
- Every answer checked against the frozen Jena reference (canonical multiset; ORDER BY/projection where declared).
  A wrong or incomplete answer is reported as such, never timed.
- Timeouts registered per suite; timeouts reported, not dropped.
- Build: wall, CPU, peak RSS, peak scratch, store size; 3 repeats ABBA where comparing variants.

## Ablations (TripleLight only)
Four vs six orders; fixed-width sorter on/off; term dedup on/off; ID-level aggregation vs decoded; planner on/off
(greedy only); WCOJ on/off; result output windowed vs old (#84).

## Laptop qualification (before renting)
1. Pin versions and configs; one script per system that builds/loads and serves; containers where available.
2. Run every suite at small scale (LUBM-100, BSBM-10M, WatDiv-10M) with answer checks, no timing claims.
3. Dry-run the full driver end to end (cold/warm switches as no-ops on macOS) and archive the output layout.
4. Write the registration (this file, frozen) and get Alex/Jianlin review.
Then: rent, provision (script), load datasets to gp3, run, archive raw data + receipts into `docs/eval-v1/`.

## Storage section (owner: Alex, from #92 msg a9c0788c)
- Memory-constrained runs: cgroup limits 8 and 16 GB on 100M–1B stores (data larger than memory); query time, peak RSS,
  block-cache hit rate (TRIPLELIGHT_TRACE_BLOCK_CACHE), bytes read.
- Cold cache via drop_caches, stores on local NVMe.
- Build comparison after #90/#91 vs QLever and Virtuoso loaders: time, peak RSS, peak scratch.
- Store size: per-file from verify, four vs six orders, next to each system's on-disk size.

## Section owners (proposed)
Storage/build/memory: Alex. Analytical + DuckDB layouts + answer gates: Jianlin. Server suites, QLever/Virtuoso/Jena
configs and the driver: WindowsXP. Claims, ablations, protocol, final registration: Kaiming.
