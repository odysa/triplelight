# Ablation-switch qualification (task #92) and derived-path coverage (pilot, task #98)

Laptop, correctness only: no timing claim. Each server was measured once, with no repeats, and the endpoints were
queried in a fixed order.

**Question 1 (#92):** do the paper's answer-preserving ablation switches return the default's answers? The switches are
`--ablate=wcoj`, `--ablate=count`, `--ablate=fast-group`, `--no-stats`, all of them together, and a six-order store.

**Question 2 (#98):** how often do benchmark queries take the expensive derived SOP/OPS paths (group regroup, or a
one-column seek memo miss), as opposed to the free twin remap? The registered kill for the pay-back materialization
idea (#95): those paths cost under 5% of query time on every suite.

## Setup

- Code: branch `kaiming/ablation-qual`, commit in `out/head`. Each dataset is built with `build-disk --build-external`
  at four orders (default) and at `--orders=6`, then wrapped as a store (`CURRENT`, `base-1`).
- Seven read-only `serve-store` endpoints, each on its own copy-on-write clone of the store, since a store directory
  admits one process. Every endpoint's `COUNT(*)` must equal the base's before any suite runs: LUBM-100 13,409,395,
  BSBM-10M 10,102,032, WatDiv-100 10,914,314 triples.
- Suites: LUBM 14 (`bench/lubm`), LUBM aggregates 30 (`docs/count-pushdown/queries/lubm`), BSBM explore 20
  (`bench/bsbm`), WatDiv 20 (`bench/watdiv`), WatDiv aggregates 42 (`docs/count-pushdown/queries/watdiv`).
  `run1.py` is `bench/run.py` with one measured run, comparing each endpoint's answer hash with the default's.
- Reproduce: `TRIPLELIGHT_DATA=<dir with lubm100.nt bsbm10.nt watdiv100.nt> bash run.sh`. It takes `/tmp/rdf-bench.lock`.

## Results

1. **Answers: 0 mismatches, 0 errors, 0 order differences** across all 126 queries × 6 variants (`out/*.txt`).
2. **Derived paths are never taken by these suites** (`out/*.derived_trace.log`):
   - 0 regroup scans and 0 memo misses in all 255 default-endpoint queries;
   - LUBM made 18 twin remaps, a two-column-prefix rewrite that costs what a kept order costs;
   - BSBM and WatDiv made none.

   So the derived-path share of time is 0%, under the 5% kill line. The pay-back materialization idea is dropped
   for these workloads. A real query-log sample, still to be found, could reopen it.
3. **Which switches matter** (single run, indicative only; the timed ablation comes from the dedicated machine):
   - **WCOJ off:** LUBM Q2 is about 26× slower, and an aggregate over Q2's pattern about 43× slower.
   - **Count shortcut off:** the LUBM aggregates are up to 250× slower; COUNT DISTINCT over Q14's pattern goes from
     0.3 ms to 80 ms.
   - **Other switches:** fast group-by off, planner statistics off and six orders stay within noise on these suites.
