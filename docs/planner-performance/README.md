# Query preparation microbenchmark

The rewrite shares immutable statistics with `Arc` and uses scalar DP arrays for
index-loop costs, cardinalities and predecessors. It builds the winning boxed
plan once. The old ordering-aware DP remains for materialized joins. A differential
test checks identical costs and plan trees for 144 graph/query combinations.
Another test checks that query-specific caches remain isolated after cloning.

The same measurement example runs seven preparations per query and reports the
median clone, exact-prefix/refinement, plan-search and total nanoseconds. Parsing,
initial data/statistics construction and executing answers are excluded. Samples
include query-stat cloning and preparation; dropping the resulting plan is outside
the timed interval. Raw per-query medians, including regressions/noise, are in the
four TSVs. Independent stage medians need not add up to the total median.

The sum of per-query median preparation times is:

| Cohort | Before | After | Ratio |
| --- | ---: | ---: | ---: |
| LUBM-10, 14 queries | 3.476 ms | 2.268 ms | 1.53x |
| WatDiv SF1, 20 queries | 97.486 ms | 49.471 ms | 1.97x |

This is one sequential comparison on a shared, variably loaded Mac. These ratios
are descriptive preparation measurements, **not end-to-end query speedups**.
Short phases approach clock resolution. Sharing saves more on WatDiv's 798
characteristic sets than on LUBM's 13. Some individual measurements may worsen;
no per-query runtime guarantee is implied. Cardinality error is unchanged.

The fixed baseline is 29ce262 with the same measurement flag copied into its
example. Separate binary copies and hashes identify old and new builds. See
`provenance.json`; the two builds use the same storage implementation. Later
storage changes are outside this comparison. Both cohorts produce byte-identical
estimate output. The new binary also matches reference counts and greedy-plan
complete result multisets on all 34 queries. The test suite passes 35 tests.

Reproduce with separate target directories for the baseline and rewrite (avoid
sharing Cargo's output artifact paths between worktrees):

```sh
cargo run --release --example optimizer_qerror -- DATA.nt QUERY_DIR REFERENCE_COUNTS.tsv --bench-planner
```

Use the saved query directories and counts in `../optimizer-lubm10/` and
`../optimizer-watdiv-sf1/`. Run the command at baseline and rewritten revisions,
copying the measurement example into the baseline worktree. Loading happens once
per cohort. Compiler and dataset provenance are recorded in those cohort folders.
