# rdfdb: a first-principles design

Status: draft. Sections 1–4 and 7 by Kaiming; sections 5–6 (workflows, storage, durability, product) by Alex.
Every number below is a measurement from `bench/results/` or the analytical-benchmark branch, with its run named.
Statements marked **[hypothesis]** are design expectations, not measurements; nothing here is a promise until the
phase that delivers it passes its gate (section 7).

## 1. What we are building

An embedded, persistent, updatable RDF database for analytical SPARQL on one machine: open a directory from Rust or
Python, load or update triples, and run joins and aggregations over 10M–1B triples with answers that equal SPARQL
semantics. The analogy is DuckDB, not a triple-store server.

Four principles decide every tradeoff, in this order:

1. **Correct answers first.** Results must equal SPARQL semantics (terms, datatypes, errors, duplicates, order),
   checked against Jena on frozen query sets before any merge. A fast wrong answer is worth nothing.
2. **Cost is rows touched × work per row, plus memory held.** Every design choice below is judged by how it changes
   one of these three.
3. **Measure, don't guess.** Same machine, same session, interleaved before/after, answer hashes on every run;
   negative results are kept.
4. **Embedded and durable is the product shape.** Speed matters inside that shape, not instead of it.

## 2. Workload and cost model

Analytical SPARQL is a small number of shapes:

| Shape | Example | What dominates |
|---|---|---|
| Star and chain joins with bound predicates | LUBM Q2/Q9, BSBM-BI q1 | join algorithm and order |
| Range filters on literal values | BI q3 (dates), BSBM q010/q013 (numbers) | rows scanned before the filter |
| GROUP BY with COUNT/SUM/AVG/MIN/MAX | BI q1–q8 | rows fed to the aggregate, per-row decode |
| ORDER BY … LIMIT k | most BI templates | sort size |
| NOT EXISTS / OPTIONAL / subqueries | BI q2, q4, q7 | evaluating the inner pattern once vs per row |

For a query, `time ≈ Σ_operators rows_in × cost_per_row + I/O`, and `peak memory ≈ resident caches + largest set of
intermediate rows alive at once`. Three levers follow, and they are the only ones:

- **Touch fewer rows**: indexes that answer the bound part of a pattern directly, filters evaluated where their
  variables are bound, the cheapest join order, and ranges answered by an index instead of a scan.
- **Do less per row**: work on integer IDs; never decode a term twice; no per-row allocation, hashing of strings or
  dynamic dispatch on the hot path.
- **Hold fewer rows**: stream through operators and materialize only where an algorithm must (hash-join build side,
  sort, aggregation state).

Measured on BSBM-BI 10M this week, rdfdb's remaining losses touch these levers. What is measured: rdfdb evaluates a
date range by scanning and testing every dated review (q3: rdfdb 0.97–1.30 s vs QLever 0.42–0.44 s, round 6), and
rdfdb's peak memory is dominated by resident blocks and materialized intermediates. **[hypothesis]** That QLever's
q1/q3 advantage comes from its value encoding is not established; we have only measured our own side (q4 peak 1.51 GB; of 807 MB live mid-query, 269 MB are index blocks
the statistics builder left in cache and ~300 MB are q4's join intermediates; `ab35`, `ab37`).

## 3. Terms and IDs: encode values, not just names

**Principle:** an ID should answer as many questions as possible without decoding.

Today a term ID is a 64-bit integer with a 4-bit kind tag; short strings (≤7 bytes) are inline, everything else is a
dictionary entry with an arbitrary number. That gives cheap equality, but no order: a date or number range filter
must decode every candidate (memoized per distinct ID), and no index range can answer it.

Design:

- **[hypothesis]** **Inline, order-preserving value IDs** for the types analytical queries compare: `xsd:integer` and its subtypes
  within 60 bits, `xsd:boolean`, `xsd:date`/`xsd:dateTime` without timezone, and `xsd:decimal`/`xsd:double` in an
  order-preserving encoding. Within one kind, ID order = value order, so `?x < c` is an index range on the POS
  permutation, and SUM/MIN/MAX read values straight out of IDs.
- **Exact term identity is kept.** Only the canonical lexical form is inlined; `"01"^^xsd:integer` or a timezoned
  date stays a dictionary term, because RDF distinguishes them as terms even when values are equal. Range scans
  therefore cover the inline range **plus** the (small, enumerable) set of non-canonical literals of that predicate,
  and the original FILTER still runs on every candidate row. Candidates are a superset by construction.
- **[hypothesis]** **Order-preserving string IDs at bulk load** (sort the vocabulary, assign IDs in order; a separate local vocabulary
  for terms added later). Then ORDER BY and string ranges also work on IDs, and the sorted vocabulary compresses with
  front coding (today the two dictionary files are 0.96 GB of a 1.2 GB 10M store).

This is a store-format change (new version; old snapshots and WALs must stay readable, and an existing dictionary term keeps its old ID: interning checks the old dictionary before assigning an inline ID, so equal terms never get two IDs), so it is a phase of
its own with Alex's durability rules, and it must first prove: RDF identity across lexical forms and datatypes,
mixed numeric promotion, the order of the dictionary versus the delta vocabulary, and old-snapshot compatibility: versioned format, no silent mixing of encodings, exact recheck for anything the
encoding does not cover. The date/time comparison semantics fix (#26) must land first, because the encoding has to
agree with the evaluator.

What we already learned: an in-memory value index on dates alone gave 1.4× on q3 (`ab33`), capped because BSBM uses one
date predicate for offers and reviews. **[hypothesis]** Value-ordered IDs make that index free (it is the POS permutation) and extend it
to numbers, where an index alone could not help q010/q013.

## 4. Execution: vectorized, pipelined operators over ID columns

**Principle:** stream batches of ID columns through operators; materialize only at algorithmic pipeline breakers.

Today the executor is row-at-a-time over fully materialized `Vec<Vec<TermId>>`: every operator returns all of its rows,
each row is its own allocation, and BGP output is copied from a flat buffer into per-row vectors. That is why q4 holds
~300 MB of intermediates, and why batching only GROUP BY's input (stage 1 of the earlier plan) traded 10% memory for
30% time (`ab36`): the batch boundary sat inside an otherwise materializing engine.

Design:

- **Data unit:** a batch of up to ~2K rows stored column-wise (one `Vec<TermId>` per variable), plus a selection vector
  for filters. No per-row allocation anywhere.
- **Operators:** index scan with prefix and ID-range bounds; merge join when both inputs are sorted on the join key
  (scans of the right permutation usually are); hash join otherwise (build the smaller side); leapfrog triejoin for
  cyclic patterns (we already have it); filter (vectorized comparisons on inline values, per-ID memo otherwise);
  hash aggregation on IDs (we have it, row-fed); top-k; semi/anti-join for EXISTS (we have it, materialized).
- **Pipelines:** scan → filter → probe → aggregate runs as one loop per batch; breakers are hash-build, sort and
  aggregation. Parallelism is morsel-driven: workers take ranges of the driving scan, and the order of results is
  restored where SPARQL semantics require it (ORDER BY, and the encounter order that floating-point SUM/AVG depend on).
- **Decode once, at the end:** only projected, returned values are turned back into RDF terms; everything before that
  is IDs (and inline values).

**[hypothesis]** for rdfdb; this is the standard design of analytical engines (DuckDB, QLever's ID tables, Umbra-style pipelines); rdfdb already
has most of the operators, but not the batch representation or the pipelining.

## 5. Optimizer

- Cardinality from statistics we already keep (per-predicate counts, characteristic sets, HLL sketches), extended with
  value histograms once values are ordered, so filter selectivity becomes visible to the planner — the parked
  filter-first experiment (`ab31`) failed exactly because the planner could not see it.
- Plan space: join order (DP for small BGPs, greedy above), join algorithm per edge (merge / hash / index nested loop /
  leapfrog), pushdown of filters and value ranges into scans, sideways information passing into subqueries.
- Our plan-quality checks at 10× scale (task #17) found current join orders already the fastest of those measured, so
  the optimizer's job grows mainly with the new physical choices above, not with re-ordering.

## 6. Storage, durability, workflows and product — Alex

### 6.1 The job and the product boundary

The target workflow is: install a library, open or create a local database, bulk-load RDF, apply updates, run SPARQL
analyses, and consume results in the host process. A second connection or server must not be required for the basic
workflow. A server is an optional compatibility adapter. Make database lifetime, cancellation, resource limits,
errors and snapshot semantics explicit in Rust and Python APIs. Supply lossless RDF terms first; Arrow batches and
Parquet/CSV are analytical interchange, not a reason to erase lexical form, language, datatype or unbound values.
Native numeric columns must be opt-in where coercion loses term identity. Python's current API materializes results;
the Arrow branch remains a prototype, not a streaming API or a zero-copy-from-storage claim.

**[Hypothesis]** Local graph analysis across heterogeneous files, incremental knowledge graphs, and dataframe
integration are useful first workflows. Interview users and test whole workflows before treating embedded RDF,
update support or any benchmark win as an exclusive advantage or defensible moat.

### 6.2 Storage and updates

Keep stable RDF term identity separate from value comparison: equal values need not be the same RDF term. Store an
immutable indexed base plus an ordered, durable WAL and a visible update overlay. Readers pin a consistent generation;
a committed update is visible atomically to later snapshots, and recovery replays acknowledged commits. Compaction
builds a new generation, validates it, and publishes it atomically; retain old generation files while readers need
them. Specify fsync, manifest publication, torn WAL, recovery and crash points as observable contracts, then test
them with process kills. Single-writer enforcement and multi-process limitations must be explicit; do not claim
concurrency beyond what is tested.

Six sorted permutations offer useful access paths, but each adds storage and ingestion work: compare six, reduced
sets and alternate layouts on actual workloads, including writes, scans, peak build scratch and first-query time.
Use block compression, checksums and lazy decoding with storage failures propagated; never turn a read failure into
an empty match. Dictionary IDs, delta IDs and inline IDs must not collide or split an existing term's identity.
Formats and WAL need explicit versions, backward-open tests, and a documented migration/rebuild path; unknown or
corrupt data must not be silently treated as an empty database. Ordered string IDs alone are not SPARQL term order:
types, language, collation and later inserts require semantic comparison or a proven compatible representation.

### 6.3 Resource and lifecycle contract

Treat cache, execution state, result buffers, active snapshots, background statistics, temporary files and concurrent
queries as parts of a single observable resource envelope, not just a cache-size flag. A configured limit should say
whether it is hard or advisory, per query or per process, and what happens at exhaustion. Bound build and compaction
scratch; support spill or return a controlled error for operators that cannot fit. Background work must not consume
unbounded cache or delay a foreground query invisibly. The uncached statistics-scan prototype is under evaluation;
its observed cache footprint is not yet a measured reduction in whole-process peak or startup latency.

Benchmark cold open/first query and warm steady state separately; report input, index size, import, updates,
compaction, process peak RSS, spill and full result consumption, as well as query time. Match cache policy and
resources for each competitor and distinguish RDF-term identity from value-compatible diagnostics. Include
larger-than-memory data and long-lived snapshots; record confidence and regressions, not only winners.

### 6.4 DuckDB lessons and unresolved decisions

DuckDB's [vectorized execution format](https://duckdb.org/docs/current/internals/vector) and [streaming and spill
model](https://duckdb.org/2024/07/09/memory-management) show why execution batches and resource management must be
designed together. This is a direction, not proof that a particular batch size or implementation will win here:
our first batched GROUP BY reduced full-BI peak about 10% while slowing it 25–33% (`ab36`). Fallbacks and hash/group
state can still materialize; cancellation, spill, snapshot isolation and error propagation need end-to-end tests.

Open decisions: which user workflows warrant optional SPARQL server support; semantic scope and named graphs; a
single resource manager versus independently budgeted caches; build/update/scan tradeoffs of index permutations;
when to add an optional value sidecar versus changing on-disk IDs; and how to expose Arrow without materializing all
IDs first. A persistent sidecar is deferred pending a workload-wide cost case. No change of IDs or query rewrite may
omit dictionary values, overlays or mixed numeric types that compare equal; a final FILTER cannot recover rows that
an incomplete index range discarded. Prove such candidate supersets with differential tests, including old snapshots,
WAL replay, updates and temporal boundaries.


## 7. Phased plan

Each phase merges only after (a) the frozen BI/LUBM/BSBM answer sets match exactly, bags and ORDER BY sequences, and
(b) a same-session benchmark against the previous main, plus QLever and DuckDB where relevant.

| Phase | Change | Expected effect **[hypothesis]** | Depends on |
|---|---|---|---|
| 0 | Statistics scan stops filling the block cache (Alex, `10ab21c6`) | −~0.27 GB baseline memory | — |
| 1 | Date/time comparison semantics fixed (#26) | correctness for timezoned values | — |
| 2 | Inline order-preserving value IDs + range scans (new store version) | q3, q010/q013; cheaper SUM/MIN/MAX | 1 |
| 3 | Batch (columnar) row representation in the executor, no per-row allocation | lower CPU and memory everywhere | — |
| 4 | Pipelined operators with morsel parallelism; merge join on sorted scans | q4-class memory; scan-heavy speed | 3 |
| 5 | Sorted vocabulary + front-coded dictionary at bulk load | dictionary size, ORDER BY on IDs | 2 |
| 6 | Value histograms in statistics; filter-aware planning | filtered joins | 2 |

Non-goals for now: distribution across machines, OWL reasoning, full-text search, SPARQL federation.

## 8. Where rdfdb stands today (measured)

- BSBM-BI 10M, same VM session, value-compatible answers on all 360 requests: rdfdb 10.8 / 7.3 s, QLever 18.3 / 18.7 s,
  DuckDB 170.7 / 113.4 s (round 6). QLever is still faster on q1 and q3 and uses about a quarter of the memory.
- LUBM-1000: rdfdb faster than QLever on every query QLever completed (round 14).
- Exact answers on the frozen sets; QLever fails strict datatype checks on 51/60 BI requests.
- Embedded Rust/Python API with WAL, overlay and compaction.

The plan targets rdfdb's two measured weaknesses, range-filter scans and materialized memory, without giving up the
first or the fourth principle. Whether it closes the gap to QLever is a hypothesis each phase must test.
