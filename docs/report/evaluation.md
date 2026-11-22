# Evaluation

This page records the historical HTTP comparisons in rounds 5, 8, 9, 11 and 12. It reports each round's
published results from [bench/results](../../bench/results). The [benchmark overview](../benchmark-overview.md)
covers the newer embedded and analytical cohorts.

## Setup

**Benchmarks.** LUBM (14 queries, no inference), WatDiv (20 basic templates) and BSBM (20 SELECT queries from
the BSBM test driver, seed 808080, captured once through a logging proxy so every system answers identical
query text). Two scales: about 10–14M triples (LUBM-100, WatDiv-100, BSBM-10M) and about 100–140M triples
(LUBM-1000: 133.6M unique triples, BSBM-100M: 98.9M).

**Systems.** TripleLight in memory and on its persistent disk store; Apache Jena TDB2 / Fuseki 6.2.0; Eclipse RDF4J
5.3.2 (native and memory stores, behind a small HTTP wrapper); Oxigraph 0.5.13; QLever (Docker image
`adfreiburg/qlever`, cache options `-c 1MB -e 1MB -E 1MB`); OpenLink Virtuoso 7 open source
(Docker image `openlink/virtuoso-opensource-7`, `NumberOfBuffers = 1,360,000`, `MaxDirtyBuffers =
1,000,000`, `ThreadsPerQuery = 10`, `ResultSetMaxRows = 10^9`, `MaxQueryExecutionTime = 0`).

**Machine.** The macOS host had 64 GB RAM. The Linux rounds used an ARM64 VM configured with 16 GB RAM
and 10 CPUs. In those rounds, QLever, Virtuoso and the Linux build of TripleLight ran in the same VM. At large
scale TripleLight used its disk store with `--cache-mb 10240`. The engines used their own buffer, cache and
index layouts.

**Method.** `bench/suite.sh` runs one system at a time under a machine-wide lock: start the server, wait until
it answers (and, for TripleLight, until background optimizer statistics are built), run every query once to warm up
and five more times, report the median, stop the server. Queries go over the SPARQL 1.1 protocol (HTTP POST,
JSON results), so times include serialization and transfer. Each answer is reduced to a canonical multiset
(sorted rows; blank-node labels erased; `typed-literal` and `xsd:string` normalized) and hashed with SHA-1;
every system is checked against Jena. Timings in the Linux VM drift by 2–3× between sessions, so only
systems measured in the same session are compared with each other.

## Results at 10–15M triples

Round 8 used one VM session with TripleLight at `334e70d`. Each cell gives the sum of query medians and the
per-query geometric mean, both in milliseconds. [LUBM](../../bench/results/round8-vm-lubm100) ·
[WatDiv](../../bench/results/round8-vm-watdiv100) · [BSBM](../../bench/results/round8-vm-bsbm10)

| | TripleLight memory | TripleLight disk | Virtuoso 7 | QLever |
|---|---|---|---|---|
| LUBM-100 | **289 / 6.8** | 378 / 8.0 | 9,441 / 23.1 | 2,103 / 15.2 |
| WatDiv-100 | **151 / 4.1** | 179 / 5.0 | 1,727 / 6.3 | 1,157 / 16.0 |
| BSBM-10M | **95** / 4.4 | 96 / **4.2** | 98 / 4.8 | 500 / 16.3 |

Round 5 used one macOS session for the native server builds. [LUBM](../../bench/results/lubm100) ·
[WatDiv](../../bench/results/watdiv100) · [BSBM](../../bench/results/bsbm10)

| | TripleLight memory | TripleLight disk | RDF4J memory | RDF4J native | Jena TDB2 | Oxigraph |
|---|---|---|---|---|---|---|
| LUBM-100 | **310 / 1.7** | 420 / 2.2 | 1,014 / 7.2 | 3,632 / 13.4 | 7,798 / 25.9 | 71,521 / 55.6 |
| WatDiv-100 | **119 / 0.8** | 141 / 1.3 | 301 / 2.8 | 879 / 5.4 | 10,198 / 9.0 | 161,338 / 57.2 |
| BSBM-10M | **27 / 0.6** | 47 / 0.9 | 113 / 2.8 | 203 / 3.1 | 1,998 / 7.7 | 1,040 / 4.3 |

TripleLight recorded the lowest summed medians in each of these six workload and session rows.

## Results at 100–140M triples

Round 11 ([LUBM-1000](../../bench/results/r11-lubm1000)) and round 12
([BSBM-100M](../../bench/results/r12-bsbm100)) used separate VM sessions.

| | TripleLight disk | Virtuoso 7 | QLever |
|---|---|---|---|
| LUBM-1000 total / geomean (ms) | **6,397 / 26.6** | 18,183* / 56.7 | 30,495 / 43.4 |
| LUBM-1000 without Q6, Q14 | **938 / 12.3** | 5,164 / 25.7 | 1,677 / 16.6 |
| LUBM-1000 fastest on | **11 / 14** | 1 | 2 |
| BSBM-100M total / geomean (ms) | 260 / **8.32** | **223** / 8.39 | 1,116 / 33.9 |
| BSBM-100M fastest on | **12 / 20** | 8 | 0 |
| LUBM-1000 memory in use (round 11) | 9.0 GB | 10.7 GB | **0.4 GB** |

\* Virtuoso's HTTP endpoint stops at 1,048,576 rows despite `ResultSetMaxRows = 10^9`, so its Q6 and Q14 answers
(7,924,765 rows each) are incomplete and its total is understated; the second row excludes those two queries.

The memory row retains the round-11 observation and its original GB units. The archived query results
do not specify the memory sampling boundary, so use it only as a reported in-use value.

On the macOS host (round 9, `bench/results/r9-*`), TripleLight in memory answers LUBM-1000 in 3.6 s and BSBM-100M in
82 ms in total.

## Answer agreement

The historical comparison reports TripleLight matches against Jena under the normalization described above. QLever's BSBM answers differ from Jena on 13 of 20
queries only because it returns values typed `xsd:integer` in the data as `xsd:int`; with that mapped, the
answers are identical. Virtuoso matches except for the two truncated LUBM-1000 results above.

## Where TripleLight is behind

- BSBM-100M q010 and q013 (numeric range filters): 100 and 38 ms against Virtuoso's 70 and 24 ms.
- Load time. Building the disk store takes 1,035 s for LUBM-1000 and 1,028–1,436 s for BSBM-100M, against
  QLever's 425 s and 139 s in these historical runs.
- Compaction (merging updates into a new base) of LUBM-1000 takes 25–33 min. On the same 138M-triple base its peak
  memory was 2.3 GB at a 2 GB cache target and 7.6 GB at 16 GB, with one run at each setting. Temporary disk use
  peaked at about 54–56 GB for a 5.3 GB store in this historical profile.

## Negative results

- Sampling-based cardinality estimation (WanderJoin-style random walks) made estimates better but routing no
  better: 3–5 of 256 walks hit on LUBM Q9, none on Q2 or WatDiv C1. Kept as an opt-in.
- Plan quality at 10× scale: forcing all 6 leapfrog orders on LUBM Q9/Q2 and 24 pattern orders on BSBM q010,
  q013 and WatDiv C3 found the chosen order fastest each time, despite q-errors up to 19,074.
- Batching the numeric FILTER's dictionary decode did not speed up disk q010; the cost was dictionary cache
  thrashing, fixed by sizing that cache from the global budget.
- Narrowing the dictionary cache lock and the seek-cursor change each moved disk Q9 by under 10%.
