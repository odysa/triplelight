# Benchmarks

TripleLight was called **rdfdb** until November 2026. The results below were measured under that name, so tables,
commit links and evidence paths still say `rdfdb`; they refer to the same engine.

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
- Each section links its raw evidence (queries, configs, per-request timings, answers).


All comparisons run every system on the same machine in the same session, with each system's answers checked
against Apache Jena. Numbers are sums of per-query medians unless stated. Lower is better.

## Analytical: BSBM Business Intelligence, 10M triples (three engines, one session)

Same Linux VM (4 CPUs, 12 GiB cap per engine), two passes in reversed order. Each number is the sum of 45
measured request times after 15 warmups, in seconds. rdfdb at main `5ac99cc`; QLever with its result cache cleared
before every request; DuckDB 2.0.0 over one lossless triples table with hand-written SQL per query template.

| | Pass 1 (s) | Pass 2 (s) | Primary answer check | COUNT int = integer | Value-only check |
|---|---:|---:|---:|---:|---:|
| **rdfdb** | **10.8** | **7.3** | 60 / 60 | 60 / 60 | 60 / 60 |
| QLever | 18.3 | 18.7 | 9 / 60 | 21 / 60 | 60 / 60 |
| DuckDB 2.0.0 | 170.7 | 113.4 | 60 / 60 | 60 / 60 | 60 / 60 |

All 360 requests pass a value-only check frozen before the run: numeric values within registered rounding bounds,
datatype ignored. The primary check also allows those numeric bounds but requires the expected datatypes, so it is
not exact term equality. QLever fails the primary check on result datatypes (computed float aggregates, and `xsd:int`
instead of `xsd:integer` for COUNT), and its passing values may differ within the rounding bounds. Its ranking
therefore holds only under the value-only rule.

Per query template, **seconds** summed over that template's requests, shown as pass 1 / pass 2 (lower is better):

| Template | q1 | q2 | q3 | q4 | q5 | q6 | q7 | q8 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| rdfdb | 0.48 / 0.31 | **0.17 / 0.11** | 1.30 / 0.97 | **5.71 / 4.06** | 2.07 / **1.20** | **0.36 / 0.21** | **0.53 / 0.32** | **0.17 / 0.10** |
| QLever | **0.26 / 0.28** | 2.06 / 2.17 | **0.42 / 0.44** | 10.89 / 11.01 | **1.78** / 1.90 | 1.88 / 1.91 | 0.54 / 0.54 | 0.44 / 0.42 |
| DuckDB | 15.82 / 9.78 | 10.45 / 6.75 | 10.32 / 6.12 | 18.30 / 12.36 | 35.62 / 23.14 | 55.07 / 32.20 | 12.57 / 11.06 | 12.56 / 11.99 |

QLever is still faster on q1 and q3, and uses much less memory: about 0.35 GiB peak server memory against rdfdb's
1.41–1.53 GiB. q7 is tied in pass 1 and lower for rdfdb in pass 2, which is not yet a stable win. rdfdb's and DuckDB's
timings varied a lot within this session (DuckDB 171 vs 113 s), while QLever's were steady. Two passes are a bounded observation, not a significance test. Virtuoso, checked in an earlier
session, returns wrong result sets for all 12 q5 instances (for example zero rows where Jena returns 14) and timed
out in its reverse pass, so it has no total. The q5 failure reproduces with small VALUES-only queries: a subquery
that projects an always-unbound join variable returns no rows in Virtuoso
([diagnostic](https://github.com/odysa/rdfdb/tree/0c9ad2b41a8bef068626ba2c6305bc6ad34c614b/docs/virtuoso-bi-unbound-diagnostic-v1)). Evidence:
[matched-vm-round6-v1](https://github.com/odysa/rdfdb/tree/ff1810232104acbde2e0885b3e4972c1189bf2c6/docs/analytical-benchmark/matched-vm-round6-v1)
(previous session: [round 5](https://github.com/odysa/rdfdb/tree/7a4f7645dc7cb839ce20e8a3c253b05adf0698d8/docs/analytical-benchmark/matched-vm-round5-v1) on `1cd8881`, rdfdb 9.4 / 10.6 s, QLever 19.4 / 17.1 s, DuckDB 118.2 / 118.3 s);
QLever value diagnostic in [value-diagnostic-v1](https://github.com/odysa/rdfdb/tree/c535ac3988f78fa16e9e1b290ed8cad84960dba5/docs/analytical-benchmark/value-diagnostic-v1);
first four-engine pass in [matched-vm-v1](https://github.com/odysa/rdfdb/tree/67f1fe7da321c67924b54c62a0be0299dba06f15/docs/analytical-benchmark/matched-vm-v1).

## Analytical at 100M: BSBM Business Intelligence, 98.9M triples (three engines, one session)

Same protocol as the 10M table above, on a 98,861,444-triple store, with a 600-second limit per request. rdfdb is at
main `ef7a778`, served from its disk store with a 10 GB cache. QLever runs with 11 GB of query memory, and DuckDB has
an 8 GB working set plus 8 GB of spill. Every container is capped at 4 CPUs and 12 GiB. The reference answers are
Jena's: 59 come from full runs and one q1 instance had to be computed per product type, labelled as such.

| | Pass 1 (s) | Pass 2 (s) | Primary | COUNT int = integer | Value-only | Peak cgroup memory |
|---|---:|---:|---:|---:|---:|---:|
| **rdfdb** | **11.7** | **6.7** | 60 / 60 | 60 / 60 | 60 / 60 | 3.7 / 4.1 GiB |
| QLever | 24.2 | 24.5 | 25 / 60 | 37 / 60 | 60 / 60 | **0.5 / 0.4 GiB** |
| DuckDB 2.0.0 | 573.8 | 443.5 | 60 / 60 | 60 / 60 | 60 / 60 | 11.4 / 12.0 GiB |

The primary check allows registered numeric error bounds; exact RDF-term equality of the full result sets holds for
58 / 42 / 25 of 60 requests (rdfdb / DuckDB / QLever). The memory column is the container's cgroup `memory.peak`,
which includes charged page cache and kernel memory, not process RSS or total RAM use. QLever is faster on q3 in both passes (2.4 / 2.5 s against rdfdb's 9.0 / 5.1 s). q3 is about 76% of rdfdb's total in
each pass and is the clearest next target. q1 changes winner between the two passes, so neither engine has a stable
lead there. QLever uses about a tenth of rdfdb's memory. rdfdb's timings vary a lot between passes. This is two
passes on a shared machine, not a general state-of-the-art claim. Evidence:
[matched-bi100-v1](https://github.com/odysa/rdfdb/tree/13123a2/docs/matched-bi100-v1).

## Aggregate queries: 72 COUNT / GROUP BY / DISTINCT queries on LUBM-100 and WatDiv (four engines, one session)

A curated suite of 72 aggregate queries (68 distinct texts) over LUBM-100 (13.4M triples) and WatDiv (10.9M
triples). It was frozen before timing, but the queries were already known to us, so this is not a blind
benchmark. All four engines ran from disk stores in the same Linux VM, one engine at a time, with 4 CPUs and 12 GiB
each. There were two passes in reversed dataset and engine order, each after a full warmup. rdfdb is at main
`9a2f49b`, and QLever's result cache was cleared before every request. Times are end-to-end HTTP, in seconds summed
over the 72 queries:

| | Pass 1 (s) | Pass 2 (s) | LUBM (s) | WatDiv (s) | Primary check | COUNT int = integer | Peak cgroup memory |
|---|---:|---:|---:|---:|---:|---:|---:|
| **rdfdb** | **0.58** | **0.54** | **0.31 / 0.31** | **0.26 / 0.23** | 72 / 72 | 72 / 72 | 0.97 / 0.94 GiB |
| Virtuoso 7 | 4.61 | 4.50 | 4.31 / 4.19 | 0.30 / 0.31 | 72 / 72 | 72 / 72 | 10.4 / 10.4 GiB |
| QLever | 5.11 | 4.60 | 2.25 / 2.14 | 2.85 / 2.47 | 0 / 72 | 72 / 72 | **0.29 / 0.29 GiB** |
| DuckDB 2.0.0 (typed-triples SQL baseline) | 73.1 | 73.6 | 48.7 / 47.2 | 24.5 / 26.4 | 72 / 72 | 72 / 72 | 3.4 / 3.4 GiB |

Totals are over the 72 query labels; the report also gives the prespecified mean-sum over the 68 distinct query
texts, with the same ordering. "Primary" here means the complete result sets match as exact RDF terms, with no
floating-point tolerance, and each score holds in both passes. The memory column is the maximum per-phase cgroup
`memory.peak` in each pass, including startup, warmup, file and kernel pages, not process RSS. OS page caches were not
cleared between phases. QLever takes 8.9× / 8.6× as long as rdfdb in total, and Virtuoso 8.0× / 8.4×. On WatDiv alone, rdfdb and Virtuoso
are close (0.26 / 0.23 s against 0.30 / 0.31 s), so a total ratio is not a win on every query. QLever's primary-check
failures are COUNT results typed `xsd:int`. QLever again uses the least memory. Two passes are a bounded observation
on a shared host, not a significance test or a general SOTA claim. Evidence:
[aggregate72-matched-results-v1](https://github.com/odysa/rdfdb/tree/87fa9e2b890b6eb2348ce30d73ac5db33325a28a/docs/aggregate72-matched-results-v1).

## Large scale: LUBM-1000 (133.6M triples) and BSBM-100M (98.9M triples)

Round 13, one Docker Linux VM session (arm64, 16 GB RAM, 10 CPUs). rdfdb runs from its disk store with a 10 GB
cache; Virtuoso 7 has a 10.6 GB buffer pool; QLever runs with its result cache disabled. Times include HTTP
transfer of JSON results. Each cell is **total time / geometric mean per query, both in milliseconds** (lower is better):

| (ms / ms) | rdfdb (disk) | QLever | Virtuoso 7 |
|---|---:|---:|---:|
| LUBM-1000, 14 queries | **6,747 / 29.0** | 42,474 / 49.1 | 31,560* / 67.1 |
| LUBM-1000 without Q6, Q14 | **726 / 13.4** | 1,903 / 18.0 | 8,471 / 28.5 |
| BSBM-100M, 20 queries | 279 / **8.6** | 1,151 / 37.9 | **240** / 9.5 |

\* Virtuoso's HTTP endpoint stops at 1,048,576 rows, so its Q6 and Q14 answers (7.9M rows each) are incomplete.

On LUBM-1000 rdfdb is the fastest system on 9 of 14 queries in round 13. In round 14, after the hash aggregation
change, rdfdb was faster than QLever on the other 12 queries (for example Q2 42.6 vs 84.0 ms, Q9 497 vs 2,561 ms).
QLever's Q6 and Q14 are not compared there: our harness could not parse their 7.9M-row JSON responses, which is a
harness limitation, not a QLever failure. Many LUBM and BSBM queries return under 30 rows, so their 5–10 ms times are
close to the HTTP request floor and say more about request overhead than about the engine. On BSBM-100M rdfdb and
Virtuoso are close: rdfdb is fastest on 15 of 20 queries, Virtuoso has the lower total. Raw results: [bench/results/r13-lubm1000](../bench/results/r13-lubm1000),
[r13-bsbm100](../bench/results/r13-bsbm100), [r14-lubm1000](../bench/results/r14-lubm1000).

## 10–15M triples, all systems

Round 8, one VM session (QLever and Virtuoso only ship Linux builds). Each cell is **total time / geometric mean per query, both in milliseconds** (lower is better):

| (ms / ms) | rdfdb memory | rdfdb disk | Virtuoso 7 | QLever |
|---|---:|---:|---:|---:|
| LUBM-100 | **289 / 6.8** | 378 / 8.0 | 9,441 / 23.1 | 2,103 / 15.2 |
| WatDiv-100 | **151 / 4.1** | 179 / 5.0 | 1,727 / 6.3 | 1,157 / 16.0 |
| BSBM-10M | **95** / 4.4 | 96 / **4.2** | 98 / 4.8 | 500 / 16.3 |

Round 5, macOS host, systems that run natively. Same units: **total ms / geometric-mean ms** per query:

| (ms / ms) | rdfdb memory | rdfdb disk | RDF4J memory | RDF4J native | Jena TDB2 | Oxigraph |
|---|---:|---:|---:|---:|---:|---:|
| LUBM-100 | **310 / 1.7** | 420 / 2.2 | 1,014 / 7.2 | 3,632 / 13.4 | 7,798 / 25.9 | 71,521 / 55.6 |
| WatDiv-100 | **119 / 0.8** | 141 / 1.3 | 301 / 2.8 | 879 / 5.4 | 10,198 / 9.0 | 161,338 / 57.2 |
| BSBM-10M | **27 / 0.6** | 47 / 0.9 | 113 / 2.8 | 203 / 3.1 | 1,998 / 7.7 | 1,040 / 4.3 |

## Embedded, in-process: rdfdb vs LadybugDB

Both through their Python APIs in one process, every result row decoded to RDF terms, no HTTP. Sum of per-query
medians (ms):

| Total (ms) | rdfdb (Python objects) | Ladybug, generic RDF mapping | Ladybug, LUBM-tuned schema |
|---|---:|---:|---:|
| LUBM-100, 14 queries | **2,835** | 14,757 | 4,484 |
| BSBM-10M, 20 queries | **27** | 4,009 | not built |

LUBM Q6 and Q14 return 796K rows each and dominate these totals; without them rdfdb takes 86 ms and tuned Ladybug
154 ms. On BSBM only Ladybug's generic mapping was measured, so the gap there reflects schema as
well as engine. Ladybug opens a store faster (30–240 ms against rdfdb's 3.1–4.3 s until statistics are ready).
Details: [embedded/README.md](embedded/README.md).

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
