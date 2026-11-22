# Paper evaluation plan (task #92), draft

Goal: every number in a research paper comes from one dedicated Linux machine, measured under a protocol registered
in advance, reproducible from an archived artifact. Scripts and answer checks are qualified on the laptop first; the
machine is rented only for the measurement window.

## Claims the evaluation must support
- C1a Embedded: through each language's in-process API (Python cohort; Java cohort), TripleLight answers the analytical
  suites faster than the embedded RDF stores and DuckDB, timed through full typed result materialization.
- C1b HTTP: as a SPARQL server, TripleLight is competitive with QLever and Virtuoso on the same suites, timed from
  request to complete response body. C1a and C1b are separate comparisons and are never mixed in one figure.
- DuckDB layout effects (single table against six sorted copies, fixed against routed) are contrasts within DuckDB
  only; a comparison with TripleLight is between registered configurations and states each one's order set.
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
- Every answer checked against the frozen Jena reference. The exact typed check is recorded for every result; the BI
  numeric and aggregate COUNT compatibility policies are declared separately and used only for ranking. Every figure
  names the gate it uses. A wrong or incomplete answer is reported as such, never timed (details: Jianlin's section).
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

## Analytical workloads, DuckDB layouts and answer gates (owner: Jianlin)

Use three analytical families. BSBM BI uses eight hand-translated SQL templates in the DuckDB arm. LUBM and WatDiv use the curated COUNT, DISTINCT and GROUP BY translations. Freeze the RDF input, graph-set cardinality, query bytes, request order and expected answers at each scale. Keep the workload names and parsed and stored triple counts in the manifest. [Prior workload and result boundaries](../benchmark-overview.md)

| Family | Laptop qualification | Dedicated Linux evaluation |
| --- | --- | --- |
| BSBM BI | The retained BSBM-10M manifest with 15 warm-up and 45 measured-position requests. Exercise all 60 with timing disabled. | Generate constants separately on each BSBM-100M and BSBM-1B graph with the pinned generator and parameter selection procedure. Freeze seeds, request mix, parameter values and all resulting SPARQL bytes before collecting comparisons. |
| LUBM aggregates | LUBM-100, 30 labels and 26 distinct texts. | Run the registered 30 labels at each admitted larger scale, with fresh Jena references from that graph. |
| WatDiv aggregates | The retained WatDiv aggregate graph, 10,914,314 stored triples and 42 labels. Identify it by the input SHA and exact count. | Run the registered 42 labels at each admitted larger scale, with fresh Jena references from that graph. |

Verify the BI parameter pools, graph membership and reference cardinalities before freezing the requests. Predeclare how the generator handles unusable constants and ties at the LIMIT boundary, then keep every selected request, including legitimately empty ones. Compute numeric diagnostic inputs and references from the exact graph at the new scale. Ratios, ordering boundaries and diagnostic anchors from 10M do not transfer to 100M or 1B.

### DuckDB layout comparison

| Arm | Physical data | Table selection |
| --- | --- | --- |
| Single table | One distinct typed triples relation from the common loader | Every pattern scans that table. |
| Six sorted copies, fixed | One database with SPO, SOP, PSO, POS, OSP and OPS copies | Every pattern scans SPO. |
| Six sorted copies, routed | The exact same database file as the fixed arm | Every pattern scans one copy. Choose the longest prefix of local constants, then break ties in the listed order. |

Freeze the BI choices at the existing scan sites. Each BI scan has a predicate constant, so the routed translation uses PSO where the fixed translation uses SPO. Filters above those scan sites stay in place. Freeze the aggregate choices from the structured triple patterns. Normalize only the table identifiers and check that the remaining SQL bytes agree across arms. Record the generated SQL and physical scan names from representative EXPLAIN plans outside timing. Apply the same memory, threads and spill settings to all three arms.

Require each sorted copy to match the control schema, row count and bag in both directions, then verify adjacent physical row order after a fresh reopen. Record full database size, each copy's creation time, checkpoint time and spill. The single-versus-fixed comparison measures the change to sorted SPO in the six-copy storage layout. The fixed-versus-routed comparison measures table selection with physical storage held fixed. DuckDB can skip row groups using min-max metadata. The six copies do not provide the same access path as TripleLight's permutation indexes. Report any comparison against TripleLight as a comparison of the registered configurations. State its order set for every result, and use the registered six-order anchor if including an order-count-matched arm. [DuckDB indexing guide](https://duckdb.org/docs/stable/guides/performance/indexing)

### Answer admission and timing

Generate frozen reference answers using a pinned Jena profile and the exact graph and query manifest for each scale. When Jena is a timed comparator, collect its reference answers in a separate unscored execution. Require the exact projected variables, binding state, RDF-term identity and row multiplicity. Check the order and LIMIT membership where declared and accept tie alternatives only under the frozen per-query policy. Empty results still need the expected projection. [SPARQL 1.1 projection and solution sequence](https://www.w3.org/TR/sparql11-query/#solutionModifiers)

- Record the primary typed check for every answer. For BI, use the established value-compatible policy for ranking only with its declared numeric columns, rounding bounds, scale-specific diagnostic anchors and cutoff/order checks. For the aggregate suite, allow the registered exact integral COUNT representation compatibility only in declared computed count columns. Group keys, multiplicity, remaining term identities and binding states stay exact. Mark each chart with its actual gate. [Prior answer policies](../benchmarks.md)
- Qualify every scheduled request before scoring a profile at that scale. Check every warm-up and measured answer again. Record timeouts, unsupported queries and mismatches as outcomes. If a profile lacks complete qualified coverage, report its coverage and any registered common-query subset explicitly. A completed subset does not become a full-suite total.
- Compare embedded Python APIs in one cohort, and Java APIs in a separate matched cohort. Time from the query or SQL-generation call through full typed result materialization in that language. Record SQL generation separately within the total. Validation and artifact serialization are outside the timer. HTTP runs use their own request-to-complete-body boundary.
- Follow the shared cold and warm protocol, recording which parts of open and statistics readiness ran before query timing. Reverse or rotate the arm order between independent passes and retain the order and every valid sample. Preserve the BI request mix and the aggregate duplicate labels. Report per-query distributions and the preregistered suite summary, with a distinct-query-text sensitivity summary for the aggregates.

### Qualification before renting

The eight-row DuckDB fixture passed all six persisted-copy checks and all 12 layout/query checks. The source audit binds all 396 SQL translations for the smaller 60 BI, 30 LUBM and 42 WatDiv requests. The next local gate is a full build and fresh reopen of each smaller layout, then an untimed replay of all arms with the archived answer policies. Exercise negative controls for a different RDF term with the same spelling, missing or unbound cells, an empty result with a missing projection and out-of-policy numeric or cutoff changes. Finish a driver dry-run with complete pass, mismatch and timeout records. Verify real cold-cache control on the dedicated machine before gathering cold measurements.

## Storage, build and memory (owner: Alex)

Supports C2 (four orders) and C3 (bulk build), and tests the disk store where it has to earn its place: data larger
than memory. Laptop evidence so far: #83 (cache budget), #88 (four against six orders), #90 (fixed-width sorter),
`docs/persistence.md` (formats and cache).

### S1. Memory-limited queries (data larger than memory)
- **Limits:** each server process runs in a cgroup v2 scope (`systemd-run --scope -p MemoryMax=… -p MemorySwapMax=0`)
  at 8 GB and 16 GB, on the 1B-scale stores (LUBM-8000, BSBM-1B, WatDiv-1B; TripleLight four-order stores are
  estimated at tens of GB, all larger than the limit). Add a smaller curve on LUBM-1000 and BSBM-100M at 2 and 4 GB.
  Same CPU set for every system.
- **TripleLight's cache setting:** its default block-cache budget is a quarter of `MemTotal`, which ignores the cgroup
  limit. Until it reads `memory.max` (a small storage change I'll make before the window), every run passes
  `--cache-mb` at half the limit, and that value is recorded. The other systems' cache or heap knobs are set to the same
  budget (WindowsXP's section).
- **Metrics per query:** cold and warm time, peak memory (`memory.peak` of the scope), bytes read from the device
  (`/proc/<pid>/io` `read_bytes`) and major faults, and TripleLight's block-cache hits, misses and bytes read per
  order (`TRIPLELIGHT_TRACE_BLOCK_CACHE`). An OOM kill is recorded as a failure, never as a time.
- **Page cache counts against the limit:** under `memory.max` the page cache is charged to the scope as well. For every
  system, and especially the mmap-based ones (QLever, Oxigraph), record `memory.stat` `anon` and `file` at the peak, so
  a reader can tell whether the limit squeezed the heap (and TripleLight's block cache) or the page cache.
- **To register before running:** the predicted warm slowdown at 16 GB and 8 GB per suite against the unlimited run,
  stated per query class (point lookups, star joins, large scans such as LUBM Q6/Q14).

### S2. Cold cache
- Stores on the instance's local NVMe (not EBS). Before every cold measurement: `sync`, drop the page cache
  (`echo 3 > /proc/sys/vm/drop_caches`), start a fresh process, wait until it is ready (TripleLight: statistics
  loaded), then time the first execution of the query.
- Also report open-to-ready time, cold and warm (#34's persisted statistics).

### S3. Bulk build
- **Loaders, same CPUs and memory limit:** `triplelight build-disk` (four orders, default) with `--orders=6` as the
  ablation; QLever's index builder; Virtuoso's bulk loader (`ld_dir` + `rdf_loader_run`); Jena `tdb2.tdbloader`;
  Oxigraph `load`. One N-Triples file per dataset, read with a cold page cache.
- **Metrics:** wall, user and system CPU, peak memory (`memory.peak`), peak scratch (directory sampled every 2 s, plus
  device `write_bytes`), final on-disk size, and the index orders each system keeps (#72 inventory).
- **TripleLight variants:** main before and after #90 (fixed-width sorter) and #91 (term dedup), three ABBA repeats on
  LUBM-1000 and BSBM-100M, one each at 1B. Other systems: three repeats at 100M scale, one at 1B.
- **Scratch sizing:** measured peak scratch is 1.7–2.1× the N-Triples input (#88), from the parallel shard term sort.
  Plan 2.1× until #91 lands: about 380 GB for a 1B dataset, plus input and store.

### S4. Store size
- TripleLight: per-file sizes from `triplelight verify`, four against six orders; every other system: `du -sb` of its
  data directory after load. Reported next to the orders or indexes each keeps, so size is compared at equal function.

### S5. Updates (optional, if time allows)
- WAL commit throughput on NVMe at batch sizes 1, 100 and 10,000 triples, with concurrent writers sharing group commit;
  compaction time and peak scratch on a 1B store with a 1% delta.

### Laptop qualification for this section
- S3 and S4 scripts for every loader at LUBM-100 and BSBM-10M: the store verifies (TripleLight) or loads and answers the
  Jena-checked suite (others). Correctness only.
- S1's cgroup harness inside the Linux Docker VM: LUBM-100 under a 2 GB limit, to check the scope, `memory.peak`,
  the I/O counters and OOM handling. No timing claims.
- S2's cache-drop step as a dry run (a no-op on macOS) so the driver path is exercised.

## Section owners
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
