# Statistics-path regression: diagnosis and rewrite

The old subset DP repeatedly enumerated all 13,113 characteristic sets. Sharing
Stats had already removed deep-clone overhead; profiling confirms cloning was
below a microsecond, while plan search consumed hundreds of milliseconds.

The rewrite precomputes eligible star-subset products once per query, shares
predicate lookups, reuses per-pattern NDVs and indexes CS signatures for rare-object
refinement. It preserves the original estimator's arithmetic and rounding order.
It does not cache results or require a warm query-plan cache. A cheap uniform-domain
greedy-work proxy skips cost planning below 100,000 approximate index operations.
The threshold is heuristic; misclassification may lose performance, never answers.

## Final profiling session

Fixed old/new binaries, one thread, five samples per query/mode. The table uses
median milliseconds. No HTTP or JSON output is included; no service throughput
claim follows from these measurements. All raw samples and phase breakdowns are
in the logs/CSVs; `summarize.py` reproduces `summary.json`.

| Query/mode | Old planning | New planning | Old query | New query |
| --- | ---: | ---: | ---: | ---: |
| F4, stats on | 442.002 | 0.020 | 442.745 | 0.320 |
| S1, stats on | 322.817 | 0.024 | 323.097 | 0.141 |

The gate keeps F4/S1 greedy. In the new binary, stats-off medians were 0.323 ms
and 0.083 ms respectively: small-query overhead remains, and clock-scale or
single-session differences are not proof of a general speedup.

C3 remains eligible (estimated work about 22 million). In the new binary its
planning median is 3.932 ms; query medians are 107.597 ms with statistics and
703.320 ms without, each returning 429,276 bindings. This is a same-binary query
API comparison, with a fixed mode order; it is not a serving benchmark or an
isolated causal estimate for each individual rewrite.

Whole-query time minus measured planning includes parsing, evaluation and result
materialization. It is labelled `outside_plan_ns`, not pure operator execution.
Independently computed phase medians need not add up to a total median.

## Correctness and limits

39 tests passed on the fixed profiling base, including 756 bit-identical
original/compiled star-subset estimates. After integrating the arena/ID-output
main and adapting the example to Options::raw_ids, all 41 tests pass.
All 14 saved LUBM-10 and all 20 WatDiv SF1 queries have identical complete result
multisets across index loops, old WCOJ and stats-enabled WCOJ, with Jena counts.
Estimate TSVs are byte-identical to the prior readouts. Q9's estimation error and
uniform-domain limitations remain; this rewrite improves preparation, not accuracy.
The small SF100 profile checks F4/S1 (zero rows) and C3; the full serving cohort
must be rerun before deciding whether statistics should default on.

`provenance.json` records source/query/binary hashes, data identity and its input-
hash limitation. `pilot-overlap/` preserves an earlier contaminated timing pilot,
excluded from this comparison. The final window was explicitly reserved in
(internal discussion) before execution. The shared Mac, sequential order and five
samples still limit statistical inference.

## Reproduction

```sh
RDFDB_PROFILE_PLAN=1 RAYON_NUM_THREADS=1 cargo run --release --example optimizer_hotpath -- DATA.nt bench/watdiv/F4.rq bench/watdiv/S1.rq bench/watdiv/C3.rq
```

Run a separately built pinned old binary for F4/S1, then the rewritten binary.
Use separate target directories or copied binaries to avoid cross-worktree Cargo
artifact reuse. Loading and initial statistics construction are outside the query
intervals. Runtime logging requires explicit opt-in and is disabled normally.

The recorded profiling source hashes precede that compatibility-only example
change; set raw_ids=false to retain the measured decoded-result API on current main.
