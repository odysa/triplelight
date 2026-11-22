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

## Server suites, QLever / Virtuoso / Jena configurations and the driver (owner: WindowsXP)

Built from what `bench/run.py`, `bench/suite.sh` and `docs/report/evaluation.md` already do, and from the problems met in
tasks #2 and #16.

**Systems, pinned.** The main tables carry TripleLight (commit recorded), QLever (Docker image by digest, run natively
where possible), Virtuoso 7 open source (image by digest), Jena TDB2 with Fuseki 6.2.0, and Oxigraph 0.5.13 (the closest
Rust peer). RDF4J 5.3.2 (native and memory stores behind the wrapper in `bench/rdf4j-server`) and DuckDB go in a second
table. Every server answers the SPARQL 1.1 protocol on one Linux machine, and versions and digests are written into every
result file.

**Equal resources, two regimes.** Every server gets the same CPUs and the same memory limit through a cgroup, not
through its own settings alone. (a) Fits in RAM: the limit is larger than the store and each system's cache or heap knob
is set to the same budget, with the knob and its value listed per system (TripleLight `--cache-mb`, QLever's memory
options, Virtuoso `NumberOfBuffers`, the JVM heap for Jena and RDF4J). (b) Memory-limited: 8 and 16 GB with stores larger
than the limit (the storage section); same configurations, only the limit changes.

**Driver.** Keep `run.py`'s protocol (one warm-up and five timed runs per query over HTTP, median) and add:
- Cold runs: restart the server and drop the page cache before one timed run of each query, recorded separately.
- Repeats: the whole session three times, with the order of systems randomized and TripleLight against QLever
  interleaved (ABBA), so drift between sessions (2 to 3× in earlier VM sessions) does not favour one system. Report the
  median and range over sessions.
- Large answers: hash the result as it streams instead of holding it (LUBM-1000 Q6 and Q14 return 7.9M rows each), and
  record rows, bytes and the time to the last byte.
- Timeout: 300 s per query, cold and warm alike, recorded as a timeout and never as a time.
- Answer classes: exact multiset match with Jena; value-compatible (QLever returns `xsd:int` for `xsd:integer`);
  incomplete (Virtuoso's HTTP endpoint stops at 1,048,576 rows despite its settings). Totals exclude an incomplete or
  timed-out query and say so.

**Loading.** One input file per dataset for every system, each system's own bulk loader (`tdb2.tdbloader`, QLever's
indexer, Virtuoso's bulk loader, Oxigraph `load`, `triplelight build-disk`) with the same CPUs. Record wall time, peak RSS,
peak scratch and the final index size, and the index orders each system keeps (the #72 inventory).

**Query sets and scales.** LUBM (14 queries, no inference) at 1000 and 8000; WatDiv basic templates at 100M and 1B; BSBM
Explore (20 queries) at 100M and 1B, with the BI templates in the analytical section. The BSBM constants must be
regenerated for the scale of the data. Disk: the planned machine (`i4i.4xlarge`, 3.75 TB of local NVMe) holds the LUBM-8000
input (about 180 GB of N-Triples) plus about 0.4 TB of build scratch (measured peak scratch in the #88 builds is 1.8 to 2.1 times the
input for six LUBM-1000 builds and 1.7 to 1.9 times for four BSBM-100M builds, so plan with 2.1×, about 380 GB for 1B triples); the gp3 volume holds the inputs only.

**Known pitfalls to design around:** Virtuoso's row cap; QLever's result typing and its result cache (disabled with
`-c 1MB -e 1MB -E 1MB` in the earlier rounds); TDB2's reliance on the OS page cache, which makes "cold" depend on the cache
drop; JVM warm-up, so Jena and RDF4J get an extra warm-up pass; and a server that is up but still building background
statistics (TripleLight's statistics-ready flag is checked before timing).

**Laptop qualification (this section's part).** Run the whole driver on LUBM-10 and BSBM-1M with every system and the
answers hashed against Jena, to check the configurations, the cold-run reset, the answer classes and the result files
before any rented hour. QLever and Virtuoso run in the Docker VM on the laptop, so those two timings are meaningless
there; only their correctness is checked.
