# TripleLight: a single-machine RDF store in Rust (design v0)

Goal (chengxu-bian, #sota-rdf-db b8e35e0d): a SPARQL store in the class of Apache Jena (TDB2) and Eclipse RDF4J,
built with current database techniques, in Rust, single machine first, benchmarked against existing products.

## 1. What the state of the art does (what we borrow)

| System | Key ideas we take |
|---|---|
| RDF-3X (Neumann & Weikum, VLDB'08/VLDBJ'10) | dictionary-encode every term to an integer ID; store all 6 SPO permutations as sorted, delta-compressed runs; aggregated (2-col, 1-col) count indexes; merge joins on sorted runs; DP join ordering |
| Hexastore (Weiss et al., VLDB'08) | 6 indexes make every triple pattern a range scan |
| QLever (Bast & Buchhold, CIKM'17) | read-optimized compressed permutations as flat column blocks with a small block index; very fast bulk load; delta-triple overlay for updates |
| Oxigraph (Rust) | inline small literals/numbers into the ID (no dictionary lookup); reuse its parsers (`oxttl`, `oxrdfio`) and SPARQL algebra (`spargebra`) |
| Leapfrog Triejoin (Veldhuizen, ICDT'14); MillenniumDB (Vrgoč et al., 2022); Free Join (Wang et al., SIGMOD'23) | worst-case-optimal joins for cyclic / many-way star patterns, mixed with binary joins where cheaper |
| Characteristic sets (Neumann & Moerkotte, ICDE'11) | cardinality estimates for star joins — the dominant SPARQL shape |
| MonetDB/X100, HyPer morsels, DuckDB | vectorized (batch-at-a-time) execution over ID columns; morsel-driven parallelism over 10 cores |
| FSST (Boncz et al., VLDB'20) | compress the string dictionary while allowing random access |

What Jena TDB2 / RDF4J native do *not* do (where we expect to win): tuple-at-a-time iterators,
B+trees on 3 (TDB2: SPO/POS/OSP + quads) or fewer orders, no WCOJ, weak cardinality estimation, single-threaded queries.

## 2. Architecture

```
SPARQL text --spargebra--> algebra --> logical plan --> optimizer (char. sets + DP/greedy, WCOJ choice)
           --> vectorized physical plan (scan/merge-join/hash-join/leapfrog/filter/agg/sort) --> decode IDs --> results
Storage:  term dictionary (ID <-> term, inline literals)  +  6 permutations (SPO SOP PSO POS OSP OPS)
          base = immutable sorted compressed column blocks (mmap)  +  delta = in-memory sorted insert/delete sets, WAL
          snapshot isolation: readers pin (base version, delta epoch); single writer; periodic merge delta -> new base
```

- **IDs**: u64. Top bits tag the kind (IRI, blank, inline int/decimal/date/bool/short string, dictionary literal).
- **Permutation blocks**: ~64K triples per block, columns delta + bit-packed; block index of first keys in RAM.
- **Named graphs**: phase 2 (add G as a fourth column: GSPO/GPOS/GOSP + default-graph union views).

## 3. Scope by phase

- **P1 (MVP):** N-Triples/Turtle bulk load, BGP + FILTER + OPTIONAL + UNION + LIMIT/ORDER/DISTINCT, SELECT/ASK, CLI + SPARQL HTTP endpoint (SPARQL 1.1 protocol, JSON results) so the benchmark drivers can hit it unchanged.
- **P2:** aggregates/GROUP BY, property paths, SPARQL Update (INSERT/DELETE DATA), named graphs, WCOJ.
- **P3:** full SPARQL 1.1 test-suite pass rate, compression tuning, parallel execution.

## 4. Benchmark plan

- **Systems:** Jena TDB2 (Fuseki), RDF4J native store (and LMDB store), Oxigraph (Rust, same parsers — the honest same-language baseline); QLever optional.
- **Workloads:** LUBM (scale 100/1000), WatDiv (10M/100M; stress mixes of star/linear/snowflake/complex), BSBM explore (with updates), SP2Bench.
- **Metrics:** bulk-load time and on-disk size; per-query latency (cold and warm, median of 5); throughput under BSBM mixes; peak RSS.
- **Correctness:** every benchmark query's result multiset must equal Jena's; W3C SPARQL 1.1 test suite pass rate reported.
- **Hygiene:** same machine (10-core Apple M-series, 64 GB), one system at a time, fixed JVM heap, page cache dropped for cold runs, report all queries (no cherry-picking), scripts + raw numbers committed.
