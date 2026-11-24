# Merge join for key-sorted probe inputs (task #105)

A pattern with one constant, one variable bound in every input row and one free variable (`?review dc:date ?date`
after a PSO scan of `?review bsbm:reviewFor ?product`) was joined by one index probe per row: 2.85M probes per BI q3
subquery on BSBM-100M. When the rows already come sorted by the bound variable and the constant's range is at most
`PROBE_ROWS` (20) times the input, each morsel of 8192 rows now reads the permutation `(constant, key, free)` once,
positioned at its first key by the new `Snapshot::scan_from` (on disk: the block directory), and walks it alongside
its rows. Per row the values come in index order and rows keep their order, so the output equals the probe join's.

Laptop ABBA, TripleLight only, main `c81bdb67` against the branch, all six profile-v2 suites (186 queries) through
the harness driver (warm, 1 warm-up and 3 timed runs per query, median), arms main, branch, branch, main. Every query's
answer class and answer hash were the same in all four arms. Geomean branch/main per suite:

| Suite | n | ratio |
| --- | ---: | ---: |
| BSBM BI | 58 | 0.977 |
| BSBM Explore | 20 | 1.017 |
| LUBM-1000 | 14 | 0.973 |
| agg-LUBM | 30 | 1.004 |
| WatDiv-100M | 20 | 0.972 |
| agg-WatDiv | 42 | 0.977 |

BI q3 per arm (ms): m002 1284.6 / 1078.7 / 1082.0 / 1262.4 (0.85), m003 1026.5 / 928.9 / 928.4 / 1009.3 (0.91),
m001 811.2 / 778.8 / 776.1 / 774.6 (no change beyond main/main spread), m000 195 in every arm (its range pushdown
plan has no sorted probe). On the CLI the m002 date join step went from 389 to 151 ms. The largest per-query ratio
above 1.1 is a sub-3 ms aggregate whose two main runs themselves differ by 10% (`summary.txt`).

Tests: `merge_join_matches_probes_in_order` (exec: duplicate keys, keys without matches, morsel boundaries inside a
key run, key as subject and as object, the declines) and positioned scans against a skipping scan on kept and derived
orders of four- and six-order bases (`derived_orders_answer_like_stored_ones`). Laptop mechanism evidence only.
