# Benchmarks

Earlier versions of TripleLight were developed as `TripleLight`; some evidence files and paths from that period keep the old
name. They refer to the same engine.

**How to read these numbers**

- **Time** is wall-clock time for the HTTP request (send query, receive the full JSON result), unless a table says
  otherwise. "Total" sums the per-request times of one pass; "geometric mean" is per query. **Lower is better.**
- **Pass 1 / Pass 2**: each benchmark runs twice, the second time with the engine order reversed, to expose
  drift on the shared machine. We report both rather than averaging them.
- **Memory** is the container's cgroup `memory.peak`, which includes page cache and kernel memory. It is not
  process RSS. **Lower is better.**
- **Answer checks** (`60 / 60` = 60 of 60 requests correct) compare every engine's results against Apache Jena.
  "Primary" requires the expected datatypes (on BSBM BI it allows registered numeric rounding
  bounds; on the aggregate suite it requires exact RDF terms); "value-only" compares numeric values within
  registered rounding bounds and ignores datatype; "COUNT int = integer" accepts `xsd:int` for COUNT results.
- **The measured DuckDB baseline uses explicit SQL mappings.** The triples are loaded into one DuckDB table
  (subject, predicate, and an object struct with kind, value, datatype and language), with no per-predicate tables,
  extra indexes or precomputed joins. On BSBM BI, a small HTTP adapter fills a hand-written SQL template for each
  query template with that request's parameters (`bi_sql.py`); on the aggregate suite, a translator written only for
  that suite's flat COUNT / GROUP BY queries turns each SPARQL query into SQL (`aggregate_sql.py`). SQL generation,
  execution and JSON encoding all run inside the timed request; the time to write the templates is not counted.
  DuckDB's answers go through the same Jena checks.
- Each section links its raw evidence (queries, configs, per-request timings, answers).


All comparisons run every system on the same machine in the same session, with each system's answers checked
against Apache Jena. Numbers are sums of per-query medians unless stated. Lower is better.

## Embedded startup with persisted statistics

With a valid statistics file beside the base (PR #36), an embedded disk database has planner statistics as soon as
open returns: 0.74–0.91 s after open start versus 20.5–25.8 s with a background scan (about 28× sooner), with lower
whole-process peak memory; the scan path itself shows no claimed change. Single registered session, descriptive
comparison, host cache uncontrolled: [report and evidence](startup36-persisted-stats/README.md).

## Mapping and store qualification outside the timing tables

The [R2RML DuckDB/SQLite tiny-control report](r2rml-tiny-qualification-v1/README.md) documents a separate,
untimed admission check. Its tested DuckDB RDF extension returns 8 instead of 9 on plain projection and both
count controls; SQLite rejects 10 of 13 unmodified DuckDB-dialect queries. The typed projection and DISTINCT
auxiliary discriminator preserve all nine terms. Neither profile qualifies for a timing total. The packet retains
the setup failure and the disclosed shared-machine lock/cache incident; these are not additional timing rows.

The [Oxilite SQLite-store tiny qualification](oxilite-tiny-qualification-v1/README.md) passes all 24
observations across memory, disk-load and separate-process disk-reopen. It has no workload timing
or full-workload admission; compiler explain output is retained separately from runtime-tracing claims.

The [RDFLib Memory tiny qualification](rdflib-tiny-qualification-v1/README.md) retains default-normalization
and lexical-preserving profiles separately. Both fail the fixed empty-result control; no workload timing
is reported, and their lexical-form results are documented without transferring earlier cohort scores.

The [embedded-store evidence inventory](embedded-store-inventory.md) distinguishes completed cohorts,
failed admission checks and unattempted candidates. Ontop and Oxilite are newly identified candidates, not
installed or measured comparators. Existing timed tables below retain their original sources and sessions.

## Embedded, in-process: Python API (BSBM BI 10M, LUBM-100 and WatDiv aggregates)

Each engine runs inside one Python process through its own API and its own on-disk store; no HTTP on any side.
Fresh container per phase (`--network none`, 4 CPUs, 12 GiB memory, container swap disabled; host/VM memory
behavior is outside that limit), two passes in reversed order. BI phases run 15 warm-up then 45 scored requests;
each aggregate phase runs one complete warm pass, then one scored pass. The timer covers query execution plus complete conversion of the result to Python values;
validation is outside the timer. Summed scored query time in **seconds**, pass 1 / pass 2 (lower is better):

| Dataset | Timed requests per pass | TripleLight | DuckDB (SQL baseline) | pyoxigraph |
|---|---:|---:|---:|---:|
| BSBM BI, 10M triples | 45 | **11.608 / 13.145** | 130.025 / 110.581 | ineligible: incomplete qualification |
| LUBM-100 aggregates | 30 | **0.666 / 0.639** | 50.397 / 40.005 | 41.701 / 34.497 |
| WatDiv aggregates | 42 | **0.195 / 0.228** | 27.069 / 18.850 | 71.611 / 71.752 |

All 1,104 scheduled requests (612 scored, 492 warm-up) passed the frozen answer gates against Apache Jena (BI:
value-compatible, including the registered numeric tolerance, exact anchors and ORDER BY rules; aggregates:
COUNT-compatible, with exact group terms and multiplicity; not exact typed-result equality on every query); no
retries, replacements, out-of-memory failures or phase caps. TripleLight ran at commit `e2e9cb2f`. Engine cache
settings differ. DuckDB answers SQL over one triples table: hand-written templates for BSBM BI, and the
aggregate suite's restricted SPARQL-to-SQL translator for LUBM and WatDiv, with generation inside the timer.
pyoxigraph 0.5.13 gets no BI total because its BI qualification timed out before completing (the 11 answers it
produced passed); in a separate nine-term
identity test it also stores `"01"^^xsd:int` and `"1"^^xsd:integer` as one term. These results are separate from the
HTTP tables below and do not establish general state of the art or full SPARQL conformance. Report, per-query
timings, memory scope and the original answers:
[embedded Python cohort](https://app.raft.build/artifacts/Azoz2CgbhGKQtHczyXUoiY).

## Embedded, in-process: Node.js API (BSBM BI 10M, LUBM-100 and WatDiv aggregates)

Same frozen workloads, gates and container limits as the Python cohort (`--network none`, 4 CPUs, 12 GiB memory,
container swap disabled), one fresh container per phase, two passes with pass 2 in reversed order. Node v24.16.0 on
Linux ARM64. The timer covers query execution plus complete conversion of the result to plain JavaScript term
objects; opening or loading the store, the stored-triple count and validation are outside it. Summed scored query
time in **seconds**, pass 1 / pass 2 (lower is better).

On-disk engines:

| Dataset | Timed requests per pass | TripleLight | quadstore 15.4.2 + Comunica 6.3.1 |
|---|---:|---:|---:|
| BSBM BI, 10M triples | 45 | **8.493 / 9.938** | no total: incomplete, and collected answers failed gates |
| LUBM-100 aggregates | 30 | **0.652 / 0.755** | no total: qualification incomplete |
| WatDiv aggregates | 42 | **0.211 / 0.262** | 322.879 / 543.624 |

In-memory engine, a separate storage group (the whole graph is loaded into memory before timing):

| Dataset | Timed requests per pass | oxigraph JS 0.5.13 |
|---|---:|---:|
| BSBM BI, 10M triples | 45 | no total: qualification incomplete |
| LUBM-100 aggregates | 30 | 16.431 / 16.179 |
| WatDiv aggregates | 42 | 76.417 / 74.671 |

All 864 scheduled requests (462 scored, 402 warm-up) passed the same frozen Jena gates as the Python cohort; no
engine errors, retries, replacements, out-of-memory failures or caps in the completed v2 attempt. TripleLight ran at
commit `2bd070ae` with its default cache target, 4,007 MiB from one quarter of the VM's reported MemTotal. oxigraph's
JS API does not expose result columns, so the adapter takes them from the frozen reference answers (values still come
from the engine); this does not independently verify the projection of empty results. oxigraph also merges literals
that RDF treats as distinct in the nine-term identity test.

Qualification outcomes behind the missing totals: quadstore BI hit its cap and, separately, some collected answers
failed the gates (q3 product selection, and q2 cutoff/order); quadstore LUBM and oxigraph BI hit their caps with every
collected answer passing.

Memory is the after-close cgroup `memory.peak` sample for each whole container phase (startup, open or load,
warm-up, scored requests and close), read after Node exits but before the container exits. It includes file-backed
(page cache) accounting and is not RSS or per-query memory; this VM reports `memory.stat` kernel 0, so no separate
kernel-memory figure is claimed. Pass 1 / pass 2 in GiB: TripleLight BI
2.00 / 2.18, LUBM 1.13 / 1.13, WatDiv 0.64 / 0.56; quadstore WatDiv 1.01 / 1.01; oxigraph LUBM 4.06 / 4.06, WatDiv
2.93 / 2.99.

Cap history: the first registered scored attempt stopped when quadstore WatDiv hit its 900-second whole-phase cap
(64 of 84 answers, all passing). A second attempt was registered before running with only that cap raised to 1,800
seconds; the first attempt is kept in the report and none of its times are used here. Missing totals mean an engine
did not complete qualification, not a zero or a measured loss. These results are separate from the Python and HTTP
tables and do not establish general state of the art or full SPARQL conformance. Report, per-query timings, the
stopped attempt and the original answers: [embedded Node.js cohort](https://app.raft.build/artifacts/BasQeph4reGyBoJpfZ69hE).

## Embedded, in-memory: Python API (TripleLight in-memory vs pyoxigraph vs DuckDB)

Every phase builds a fresh in-memory store from the original N-Triples file inside its own container: TripleLight
`Database.in_memory()` and native `load()` (commit `fd204cec`), waiting for statistics readiness; pyoxigraph 0.5.13
pathless `Store()` with its in-memory bulk loader; DuckDB 2.0.0 `:memory:` with the existing typed-triples SQL schema and
translator (an SQL baseline, not a SPARQL engine). Same frozen workloads, references and gates as the other embedded
cohorts. One registered attempt of 16 fresh containers (Python 3.11.18, Linux ARM64, 4 CPUs, 12 GiB with container swap
disabled, network none, read-only root): pass 1 = TL, pyoxigraph, DuckDB on LUBM, then on WatDiv, then TL and DuckDB on BI;
pass 2 in exactly the reverse order. Requests run back to back and answers are validated at each phase boundary. The timer
covers the API call and full conversion to typed RDF bindings (plus SQL generation for DuckDB); loading, statistics
readiness, the stored-triple count and validation are outside it. Summed scored query time in **seconds**, pass 1 / pass 2
(lower is better):

| Dataset | Timed requests per pass | TripleLight in-memory | pyoxigraph in-memory | DuckDB in-memory (SQL baseline) |
|---|---:|---:|---:|---:|
| LUBM-100 aggregates | 30 | **0.507 / 0.432** | 9.227 / 6.610 | 23.406 / 19.644 |
| WatDiv aggregates | 42 | **0.258 / 0.199** | 114.681 / 45.501 | 13.412 / 11.635 |
| BSBM BI, 10M triples | 45 | **18.858 / 17.435** | excluded: qualification incomplete | 29.293 / 29.477 |

All 1,104 scheduled requests (612 scored, 492 warm-up) passed the frozen Jena gates; no cap hits, out-of-memory kills,
request errors, retries or discarded phases. Every phase had a 1,800-second whole-phase cap set before the run.
pyoxigraph BI was excluded after its in-memory qualification reached that cap with 13 passing answers and 47 unfinished;
it was not retried. pyoxigraph's WatDiv passes differ by 2.52×, dominated by the two C3 count queries; no cause is
established. DuckDB is internally limited to 8 GB (decimal; reported 7.4 GiB) with spilling disabled, while TripleLight
and pyoxigraph have no internal ceiling beyond the shared 12 GiB container limit.

Whole-phase cgroup `memory.peak` (startup, load, warm-up, scored requests and close; not RSS), pass 1 / pass 2 in GiB:
TripleLight LUBM 9.33 / 8.10, WatDiv 6.73 / 6.72, BI 8.99 / 9.06; pyoxigraph LUBM 6.21 / 7.66, WatDiv 5.82 / 5.82; DuckDB
LUBM 4.05 / 4.07, WatDiv 4.74 / 4.94, BI 3.98 / 4.01. TripleLight's phases also show substantial cgroup page reclaim over
the whole phase; without counters at the query boundaries, this does not establish an effect on query time.

This group is separate from the Node.js in-memory, on-disk Python, JVM and HTTP results (different runtimes, storage,
source commits or sessions). Report, per-query timings, memory and reclaim counters, and the original answers:
[embedded Python in-memory cohort](https://app.raft.build/artifacts/JFouozmVAo5HZK6JCQe8bg).

## Embedded, in-memory: Node.js API (TripleLight in-memory vs oxigraph JS)

Both engines build a fresh in-memory store from the N-Triples file inside each container: TripleLight with
`Database.inMemory()` and `load()` through its native Node binding (commit `fd204cec`), oxigraph JS 0.5.13 through its
WASM package. Same frozen workloads, references and gates as the other embedded cohorts. One registered attempt, ten
fresh containers (4 CPUs, 12 GiB with container swap disabled, network none, read-only root), pass 1 = TL LUBM, oxigraph
LUBM, TL WatDiv, oxigraph WatDiv, TL BI, and pass 2 in exactly the reverse order. Requests run back to back; answers
are validated at each phase boundary. The timer covers the API call and full conversion to typed JavaScript terms;
loading, TripleLight's statistics readiness, the stored-triple count and validation are outside it. Summed scored
query time in **seconds**, pass 1 / pass 2 (lower is better):

| Dataset | Timed requests per pass | TripleLight in-memory | oxigraph JS |
|---|---:|---:|---:|
| LUBM-100 aggregates | 30 | **0.686 / 0.496** | 14.122 / 14.653 |
| WatDiv aggregates | 42 | **0.188 / 0.291** | 83.609 / 77.859 |
| BSBM BI, 10M triples | 45 | 14.277 / 13.341 (standalone) | excluded: earlier qualification incomplete |

All 696 scheduled requests (378 scored, 318 warm-up) passed the frozen Jena gates; no cap hits, out-of-memory kills,
retries or discarded phases. oxigraph BI was not reattempted: its earlier Node qualification stopped at its
1,800-second cap after 11 passing answers. Whole-phase caps were set before the run: 1,800 seconds for TripleLight and
3,600 seconds for oxigraph, whose cap was raised because its LUBM qualification took 1,087 seconds. oxigraph's JS API does not report result columns, so the adapter takes them
from the reference answers (empty-result projection is not independently verified).

Whole-phase cgroup `memory.peak` (startup, load, warm-up, scored requests and close; not RSS), pass 1 / pass 2 in GiB:
TripleLight LUBM 8.07 / 9.45, WatDiv 5.49 / 6.05, BI 7.83 / 8.80; oxigraph LUBM 4.05 / 4.02, WatDiv 2.93 / 2.95.
TripleLight load took 29–44 seconds per phase, plus 2.4–3.3 seconds until statistics were ready. TripleLight uses
roughly twice oxigraph's peak memory here; the cause has not been profiled yet.

This group is separate from the on-disk Node, Python, JVM and HTTP results (different storage, source commit and
session); in particular it does not measure TripleLight in-memory against TripleLight on disk. Report, per-query
timings and the original answers: [embedded Node.js in-memory cohort](https://app.raft.build/artifacts/HtwKaT16usNuL5hiV2kMH8).

## Analytical: BSBM Business Intelligence, 10M triples (three engines, one session)

Same Linux VM (4 CPUs, 12 GiB cap per engine), two passes in reversed order. Each number is the sum of 45
measured request times after 15 warmups, in seconds. TripleLight at main `5ac99cc`; QLever with its result cache cleared
before every request; DuckDB 2.0.0 (no SPARQL) answers hand-written SQL templates, one per query template, over one lossless triples table.

| | Pass 1 (s) | Pass 2 (s) | Primary answer check | COUNT int = integer | Value-only check |
|---|---:|---:|---:|---:|---:|
| **TripleLight** | **10.8** | **7.3** | 60 / 60 | 60 / 60 | 60 / 60 |
| QLever | 18.3 | 18.7 | 9 / 60 | 21 / 60 | 60 / 60 |
| DuckDB 2.0.0 (hand-written SQL) | 170.7 | 113.4 | 60 / 60 | 60 / 60 | 60 / 60 |

All 360 requests pass a value-only check frozen before the run: numeric values within registered rounding bounds,
datatype ignored. The primary check also allows those numeric bounds but requires the expected datatypes, so it is
not exact term equality. QLever fails the primary check on result datatypes (computed float aggregates, and `xsd:int`
instead of `xsd:integer` for COUNT), and its passing values may differ within the rounding bounds. Its ranking
therefore holds only under the value-only rule.

Per query template, **seconds** summed over that template's requests, shown as pass 1 / pass 2 (lower is better):

| Template | q1 | q2 | q3 | q4 | q5 | q6 | q7 | q8 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| TripleLight | 0.48 / 0.31 | **0.17 / 0.11** | 1.30 / 0.97 | **5.71 / 4.06** | 2.07 / **1.20** | **0.36 / 0.21** | **0.53 / 0.32** | **0.17 / 0.10** |
| QLever | **0.26 / 0.28** | 2.06 / 2.17 | **0.42 / 0.44** | 10.89 / 11.01 | **1.78** / 1.90 | 1.88 / 1.91 | 0.54 / 0.54 | 0.44 / 0.42 |
| DuckDB (hand-written SQL) | 15.82 / 9.78 | 10.45 / 6.75 | 10.32 / 6.12 | 18.30 / 12.36 | 35.62 / 23.14 | 55.07 / 32.20 | 12.57 / 11.06 | 12.56 / 11.99 |

QLever is still faster on q1 and q3, and uses much less memory: about 0.35 GiB peak server memory against TripleLight's
1.41–1.53 GiB. q7 is tied in pass 1 and lower for TripleLight in pass 2, which is not yet a stable win. TripleLight's and DuckDB's
timings varied a lot within this session (DuckDB 171 vs 113 s), while QLever's were steady. Two passes are a bounded observation, not a significance test. Virtuoso, checked in an earlier
session, returns wrong result sets for all 12 q5 instances (for example zero rows where Jena returns 14) and timed
out in its reverse pass, so it has no total. The q5 failure reproduces with small VALUES-only queries: a subquery
that projects an always-unbound join variable returns no rows in Virtuoso
([diagnostic](https://github.com/odysa/triplelight/tree/0c9ad2b41a8bef068626ba2c6305bc6ad34c614b/docs/virtuoso-bi-unbound-diagnostic-v1)). Evidence:
[matched-vm-round6-v1](https://github.com/odysa/triplelight/tree/ff1810232104acbde2e0885b3e4972c1189bf2c6/docs/analytical-benchmark/matched-vm-round6-v1)
(previous session: [round 5](https://github.com/odysa/triplelight/tree/7a4f7645dc7cb839ce20e8a3c253b05adf0698d8/docs/analytical-benchmark/matched-vm-round5-v1) on `1cd8881`, TripleLight 9.4 / 10.6 s, QLever 19.4 / 17.1 s, DuckDB 118.2 / 118.3 s);
QLever value diagnostic in [value-diagnostic-v1](https://github.com/odysa/triplelight/tree/c535ac3988f78fa16e9e1b290ed8cad84960dba5/docs/analytical-benchmark/value-diagnostic-v1);
first four-engine pass in [matched-vm-v1](https://github.com/odysa/triplelight/tree/67f1fe7da321c67924b54c62a0be0299dba06f15/docs/analytical-benchmark/matched-vm-v1).

## Analytical at 100M: BSBM Business Intelligence, 98.9M triples (three engines, one session)

Same protocol as the 10M table above, on a 98,861,444-triple store, with a 600-second limit per request. TripleLight is at
main `ef7a778`, served from its disk store with a 10 GB cache. QLever runs with 11 GB of query memory, and DuckDB (hand-written SQL templates, as above) has
an 8 GB working set plus 8 GB of spill. Every container is capped at 4 CPUs and 12 GiB. The reference answers are
Jena's: 59 come from full runs and one q1 instance had to be computed per product type, labelled as such.

| | Pass 1 (s) | Pass 2 (s) | Primary | COUNT int = integer | Value-only | Peak cgroup memory |
|---|---:|---:|---:|---:|---:|---:|
| **TripleLight** | **11.7** | **6.7** | 60 / 60 | 60 / 60 | 60 / 60 | 3.7 / 4.1 GiB |
| QLever | 24.2 | 24.5 | 25 / 60 | 37 / 60 | 60 / 60 | **0.5 / 0.4 GiB** |
| DuckDB 2.0.0 (hand-written SQL) | 573.8 | 443.5 | 60 / 60 | 60 / 60 | 60 / 60 | 11.4 / 12.0 GiB |

The primary check allows registered numeric error bounds; exact RDF-term equality of the full result sets holds for
58 / 42 / 25 of 60 requests (TripleLight / DuckDB / QLever). The memory column is the container's cgroup `memory.peak`,
which includes charged page cache and kernel memory, not process RSS or total RAM use. QLever is faster on q3 in both passes (2.4 / 2.5 s against TripleLight's 9.0 / 5.1 s). q3 is about 76% of TripleLight's total in
each pass and is the clearest next target. q1 changes winner between the two passes, so neither engine has a stable
lead there. QLever uses about a tenth of TripleLight's memory. TripleLight's timings vary a lot between passes. This is two
passes on a shared machine, not a general state-of-the-art claim. Evidence:
[matched-bi100-v1](https://github.com/odysa/triplelight/tree/13123a2/docs/matched-bi100-v1).

A separate untimed Virtuoso100M correctness screen completed all 60 requests, passing 42 primary,
42 COUNT-compatible and 59 value-compatible checks. One warmup q5 returned 0 rows against 25 reference rows;
the other eleven q5 references were empty. Of the 18 primary/COUNT failures, 17 are output-datatype failures
(16 q6 and `m000_p04_q4` lack the required `xsd:float` type); all 17 pass the value and ordering checks.
The remaining failure is the q5 bag mismatch. The frozen eligibility rule includes warmups, so no Virtuoso total
or speed ratio is added. Original import receipts are also missing, and this four-thread screen is separate
from the historical performance cohort. [Report and portable raw evidence](virtuoso100-qualification-v1/README.md).

## Aggregate queries: 72 COUNT / GROUP BY / DISTINCT queries on LUBM-100 and WatDiv (four engines, one session)

A curated suite of 72 aggregate queries (68 distinct texts) over LUBM-100 (13.4M triples) and WatDiv (10.9M
triples). It was frozen before timing, but the queries were already known to us, so this is not a blind
benchmark. All four engines ran from disk stores in the same Linux VM, one engine at a time, with 4 CPUs and 12 GiB
each. There were two passes in reversed dataset and engine order, each after a full warmup. TripleLight is at main
`9a2f49b`, and QLever's result cache was cleared before every request. Times are end-to-end HTTP, in seconds summed
over the 72 queries:

| | Pass 1 (s) | Pass 2 (s) | LUBM (s) | WatDiv (s) | Primary check | COUNT int = integer | Peak cgroup memory |
|---|---:|---:|---:|---:|---:|---:|---:|
| **TripleLight** | **0.58** | **0.54** | **0.31 / 0.31** | **0.26 / 0.23** | 72 / 72 | 72 / 72 | 0.97 / 0.94 GiB |
| Virtuoso 7 | 4.61 | 4.50 | 4.31 / 4.19 | 0.30 / 0.31 | 72 / 72 | 72 / 72 | 10.4 / 10.4 GiB |
| QLever | 5.11 | 4.60 | 2.25 / 2.14 | 2.85 / 2.47 | 0 / 72 | 72 / 72 | **0.29 / 0.29 GiB** |
| DuckDB 2.0.0 (SQL from a suite-specific translator) | 73.1 | 73.6 | 48.7 / 47.2 | 24.5 / 26.4 | 72 / 72 | 72 / 72 | 3.4 / 3.4 GiB |

Totals are over the 72 query labels; the report also gives the prespecified mean-sum over the 68 distinct query
texts, with the same ordering. "Primary" here means the complete result sets match as exact RDF terms, with no
floating-point tolerance, and each score holds in both passes. The memory column is the maximum per-phase cgroup
`memory.peak` in each pass, including startup, warmup, file and kernel pages, not process RSS. OS page caches were not
cleared between phases. QLever takes 8.9× / 8.6× as long as TripleLight in total, and Virtuoso 8.0× / 8.4×. On WatDiv alone, TripleLight and Virtuoso
are close (0.26 / 0.23 s against 0.30 / 0.31 s), so a total ratio is not a win on every query. QLever's primary-check
failures are COUNT results typed `xsd:int`. QLever again uses the least memory. Two passes are a bounded observation
on a shared host, not a significance test or a general SOTA claim. Evidence:
[aggregate72-matched-results-v1](https://github.com/odysa/triplelight/tree/87fa9e2b890b6eb2348ce30d73ac5db33325a28a/docs/aggregate72-matched-results-v1).

## Large scale: LUBM-1000 (133.6M triples) and BSBM-100M (98.9M triples)

Round 13, one Docker Linux VM session (arm64, 16 GB RAM, 10 CPUs). TripleLight runs from its disk store with a 10 GB
cache; Virtuoso 7 has a 10.6 GB buffer pool; QLever runs with its result cache disabled. Times include HTTP
transfer of JSON results. Each cell is **total time / geometric mean per query, both in milliseconds** (lower is better):

| (ms / ms) | TripleLight (disk) | QLever | Virtuoso 7 |
|---|---:|---:|---:|
| LUBM-1000, 14 queries | **6,747 / 29.0** | 42,474 / 49.1 | 31,560* / 67.1 |
| LUBM-1000 without Q6, Q14 | **726 / 13.4** | 1,903 / 18.0 | 8,471 / 28.5 |
| BSBM-100M, 20 queries | 279 / **8.6** | 1,151 / 37.9 | **240** / 9.5 |

\* Virtuoso's HTTP endpoint stops at 1,048,576 rows, so its Q6 and Q14 answers (7.9M rows each) are incomplete.

On LUBM-1000 TripleLight is the fastest system on 9 of 14 queries in round 13. In round 14, after the hash aggregation
change, TripleLight was faster than QLever on the other 12 queries (for example Q2 42.6 vs 84.0 ms, Q9 497 vs 2,561 ms).
QLever's Q6 and Q14 are not compared there: our harness could not parse their 7.9M-row JSON responses, which is a
harness limitation, not a QLever failure. Many LUBM and BSBM queries return under 30 rows, so their 5–10 ms times are
close to the HTTP request floor and say more about request overhead than about the engine. On BSBM-100M TripleLight and
Virtuoso are close: TripleLight is fastest on 15 of 20 queries, Virtuoso has the lower total. Raw results: [bench/results/r13-lubm1000](../bench/results/r13-lubm1000),
[r13-bsbm100](../bench/results/r13-bsbm100), [r14-lubm1000](../bench/results/r14-lubm1000).

## 10–15M triples, all systems

Round 8, one VM session (QLever and Virtuoso only ship Linux builds). Each cell is **total time / geometric mean per query, both in milliseconds** (lower is better):

| (ms / ms) | TripleLight memory | TripleLight disk | Virtuoso 7 | QLever |
|---|---:|---:|---:|---:|
| LUBM-100 | **289 / 6.8** | 378 / 8.0 | 9,441 / 23.1 | 2,103 / 15.2 |
| WatDiv-100 | **151 / 4.1** | 179 / 5.0 | 1,727 / 6.3 | 1,157 / 16.0 |
| BSBM-10M | **95** / 4.4 | 96 / **4.2** | 98 / 4.8 | 500 / 16.3 |

Round 5, macOS host, systems that run natively. Same units: **total ms / geometric-mean ms** per query:

| (ms / ms) | TripleLight memory | TripleLight disk | RDF4J memory | RDF4J native | Jena TDB2 | Oxigraph |
|---|---:|---:|---:|---:|---:|---:|
| LUBM-100 | **310 / 1.7** | 420 / 2.2 | 1,014 / 7.2 | 3,632 / 13.4 | 7,798 / 25.9 | 71,521 / 55.6 |
| WatDiv-100 | **119 / 0.8** | 141 / 1.3 | 301 / 2.8 | 879 / 5.4 | 10,198 / 9.0 | 161,338 / 57.2 |
| BSBM-10M | **27 / 0.6** | 47 / 0.9 | 113 / 2.8 | 203 / 3.1 | 1,998 / 7.7 | 1,040 / 4.3 |

## Embedded, in-process: JVM baselines (Jena and RDF4J native Java APIs)

TripleLight has no Java binding in this evaluation, so these are JVM baselines on the same frozen workloads, references
and gates; they do not rank TripleLight across runtimes. Jena 6.2.0 and RDF4J 5.3.2 through their native APIs on OpenJDK
21.0.13 (G1, `-Xmx8g`, four active processors), Linux ARM64. One registered attempt of 18 fresh containers (4 CPUs,
12 GiB with container swap disabled, network none; the root file system was not read-only, unlike the Python in-memory
run); pass 2 is the exact reverse of pass 1. The timer covers query compilation, API execution and full conversion to
typed RDF bindings; opening or import, the stored-triple count, warm-ups and validation are outside it. Each answer is
serialized and validated before the next request is sent; those gaps are recorded and published, and they can give
background JIT and GC work different opportunities than the back-to-back Python and Node cohorts. Summed scored query
time in **seconds**, pass 1 / pass 2 (lower is better).

Persistent stores, aggregate workloads:

| Dataset | Timed requests per pass | Jena TDB2 | RDF4J NativeStore | RDF4J LMDBStore |
|---|---:|---:|---:|---:|
| LUBM-100 aggregates | 30 | 9.503 / 18.807 | 9.791 / 14.568 | 6.640 / 13.800 |
| WatDiv aggregates | 42 | 20.572 / 39.844 | 1.344 / 2.653 | 1.362 / 1.542 |

Jena TDB2 and NativeStore start from private copies of prepared stores; LMDBStore imports the N-Triples file fresh in
every phase. That difference in preparation and cache history limits comparisons between them.

Standalone baselines:

| Engine | Dataset | Timed requests per pass | Query seconds |
|---|---|---:|---:|
| Jena TDB2 | BSBM BI, 10M triples | 45 | 148.849 / 188.820 |
| RDF4J MemoryStore (in memory) | LUBM-100 aggregates | 30 | 1.944 / 3.380 |
| RDF4J MemoryStore (in memory) | WatDiv aggregates | 42 | 0.495 / 0.559 |

All 1,272 scheduled requests (666 scored, 606 warm-up) passed the frozen Jena gates; no cap hits, out-of-memory kills,
request errors or cleanup failures. Every phase had a 1,800-second whole-phase cap set before the run. All three RDF4J
backends were excluded from BI before scoring: each disagrees with Jena on the 12 instances of query q5, which joins on an
unbound GROUP BY key. A separate one-parameter NativeStore diagnostic (one q5 instance) restored agreement when that key
was removed; it does not establish agreement across all 12 instances or the other backends, and does not requalify the
original query. In a separate small identity test, Jena TDB2 stores `"01"^^xsd:int` as `"1"`. Pass 2 is noticeably slower for several pairs (Jena aggregates,
RDF4J NativeStore and LMDBStore on LUBM); both passes are kept and no cause is established.

Whole-phase cgroup `memory.peak` (startup, open or import, warm-up, scored requests, close; not heap or RSS), pass 1 /
pass 2 in GiB: Jena BI 3.17 / 3.44, LUBM 4.48 / 4.17, WatDiv 2.08 / 2.01; NativeStore LUBM 4.59 / 4.66, WatDiv 1.08 / 1.49;
MemoryStore LUBM 8.87 / 8.68, WatDiv 5.16 / 5.45; LMDBStore LUBM 9.85 / 9.88, WatDiv 10.25 / 10.27.

This cohort is separate from every Python, Node.js and HTTP table, and from the stopped first JVM attempt. Report,
per-query timings, request gaps and the original answers: [embedded JVM cohort](https://app.raft.build/artifacts/4FiCDWgEyXpz3b3fRS8GLk).

## Embedded, in-process: HDT baseline (HDT 3.0.10 through Jena 4.3.2, standalone)

HDT 3.0.10 supplies a compressed, read-only triple store through its Jena Graph adapter, which pins Jena 4.3.2; it
shares Jena's query engine and is not an independent SPARQL implementation. It ran as its own registered attempt in a
separate session from the JVM baselines above, so its totals are not a same-session or version-upgrade comparison with
Jena TDB2 6.2.0, and like them they do not rank TripleLight. Same frozen workloads, references and gates, and the same
JVM limits: OpenJDK 21.0.13, G1, `-Xmx8g`, four active processors, 4 CPUs and 12 GiB with container swap disabled,
network none (root not read-only). Six fresh containers; pass 1 = BI, LUBM, WatDiv and pass 2 in the reverse order.
Each phase opens a private copy of an HDT store generated and indexed beforehand; generation is not part of this run, and
no OS cache reset is claimed. The timer covers parsing and compilation, execution and full conversion to typed RDF
bindings. Each answer is validated before the next request (gaps published), as in the JVM baselines. Summed scored query
time in **seconds**, pass 1 / pass 2 (lower is better):

| Dataset | Timed requests per pass | HDT 3.0.10 / Jena 4.3.2 |
|---|---:|---:|
| LUBM-100 aggregates | 30 | 9.253 / 9.789 |
| WatDiv aggregates | 42 | 11.445 / 14.543 |
| BSBM BI, 10M triples | 45 | 164.705 / 171.270 |

All 408 scheduled requests (234 scored, 174 warm-up) passed the frozen Jena gates, with no cap hits, out-of-memory kills,
request errors or cleanup failures. Whole-phase cgroup `memory.peak` (opening, count, warm-up, scored requests and close;
not heap or RSS), pass 1 / pass 2 in GiB: BI 1.84 / 2.13, LUBM 3.34 / 3.34, WatDiv 0.58 / 0.59. Report, per-query timings,
request gaps and the original answers: [embedded HDT cohort](https://app.raft.build/artifacts/Tbn4rJhupS6fZNUv9M8zb1).

## Embedded, in-process: TripleLight vs LadybugDB

Both through their Python APIs in one process, every result row decoded to RDF terms, no HTTP. Sum of per-query
medians (ms):

| Total (ms) | TripleLight (Python objects) | Ladybug, generic RDF mapping | Ladybug, LUBM-tuned schema |
|---|---:|---:|---:|
| LUBM-100, 14 queries | **2,835** | 14,757 | 4,484 |
| BSBM-10M, 20 queries | **27** | 4,009 | not built |

LUBM Q6 and Q14 return 796K rows each and dominate these totals; without them TripleLight takes 86 ms and tuned Ladybug
154 ms. In this historical session, BSBM used only Ladybug's generic mapping, so that gap reflects schema as
well as engine. Ladybug opens a store faster (30–240 ms against TripleLight's 3.1–4.3 s until statistics are ready).
Details: [embedded/README.md](embedded/README.md).

A later **within-Ladybug schema experiment** covers all 20 frozen BSBM10M Explore instances. In two recorded
warm-cache passes, query-to-full-SPARQL-JSON totals were **19.924 / 19.436 s** for the generic mapping and
**4.519 / 4.424 s** for a hybrid schema (4.41× / 4.39× lower time). All 120 scheduled responses, including warmup,
pass typed bags; all 54 declared order checks pass. The hybrid retains literal edges for complex OPTIONAL:
its database is **58.9% larger**, its maximum query/export worker RSS is about **25.6% higher**, and its warm-cache
opens are slower. Gains vary: q011/q012/q014/q015 improve about 1.3×; the largest gains are around 20×.
This experiment uses fresh Python processes per request and a different output boundary from
the historical table above; **the times must not be combined into a cross-engine ranking**. The
[report and complete portable evidence](ladybug-hybrid-explore-v1/README.md) include both passes, import/storage
costs, every query, and the earlier pure-column limitation and q023 ordering failure.


## Where TripleLight is behind

- BI q3 against QLever, at 10M and at 100M. q1 is also slower at 10M; at 100M its winner changes between passes.
- Memory, measured two ways: on the 10M BI run, peak server memory (process) is about 4× QLever's; on the 100M BI
  run, peak cgroup memory (which includes page cache) is 3.7–4.1 GiB against QLever's 0.4–0.5 GiB.
- Loading: building the LUBM-1000 disk store took 1,035 s against QLever's 425 s in round 9. A parallel loader has
  been merged since, but its speedup is not established and it has not been compared at that scale.
- Numeric range: `xsd:decimal` is exact within 96 bits and 28 fractional digits and `xsd:integer` within 64 bits;
  results outside that range are query errors (Jena uses arbitrary precision). This includes decimal underflow,
  which is stricter than XPath (XPath returns 0.0 on underflow, and only overflow is an error), so this is not full
  SPARQL numeric conformance. Non-terminating decimal quotients are rounded to 28 digits.
- SPARQL coverage: property paths, named graphs (`GRAPH`, `FROM`), `SERVICE` and some functions (hashes, `NOW`,
  `UUID`, …) are rejected as unsupported. W3C SPARQL 1.1 query suite: 281 pass, 6 fail, 44 unsupported
  ([details](../bench/results/w3c-sparql11)).
- Store open: statistics take seconds to become ready after opening a store (42.8 s for the 100M BSBM store).

Full method and earlier rounds (8–12, so some totals differ from the later rounds above):
[report/evaluation.md](report/evaluation.md) and
[bench/README.md](../bench/README.md).
