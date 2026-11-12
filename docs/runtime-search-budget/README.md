# C1/C2: preparation cost and a bounded runtime policy

The SF100 API diagnosis separates complete `planned_order` preparation from the
whole query. Fixed binaries, one thread, five sequential samples per mode, no HTTP
or JSON serialization. The original statistics-enabled medians were:

| Query | Preparation | Whole query | Stats-off whole query |
| --- | ---: | ---: | ---: |
| C1 | 6.93 ms | 10.63 ms | 2.95 ms |
| C2 | 9.48 ms | 15.73 ms | 8.83 ms |

Most regression is preparation. The query time outside planning includes parsing,
execution and materialization; it was slightly higher for C1 and lower for C2.
These measurements do not isolate individual operators or prove every chosen
join order is worse.

## Discarded candidate

A candidate capped DP at seven patterns but used the statistics-cost connected
greedy search above that. It still compiled star statistics and produced a much
slower C2 execution in the observed session. We discarded it. Its raw log and
summary remain for audit; Kaiming reported a possibly overlapping release build,
so these timings are not an interference-free causal comparison. No speedup is
claimed from them.

## Final policy

Above seven patterns (more than 128 binary-DP subset states), the server now
returns directly to its original runtime exact-index-count greedy order, or its
legacy trie order for cyclic BGPs. This happens before count refinement and star
compilation. C3's six-pattern optimizer path remains eligible. Library users retain
the original configurable DP and connected-greedy APIs.

This trades potential optimization of wide queries for bounded server preparation.
It is a structural fallback to the existing reference path, not a claim that wide
queries cannot benefit from optimization. The final policy has no newly measured
post-change SF100/HTTP timings here. Tests include an eight-pattern, high estimated
work case with 25,000 complete bindings compared against the unoptimized runtime.
All 44 tests passed on the diagnosis base before integrating concurrent store work.

`summary.json`, CSVs and raw logs preserve every sample; `summarize.py` reproduces
the medians. Source/query/binary provenance and timing limitations are recorded in
`provenance.json`. The persistence work has priority, so this investigation stops
after the bounded policy and regression checks rather than starting another full
benchmark round.

After rebasing onto the concurrent durable-store implementation, all 50 integrated
`cargo test --all-targets` tests pass, including crash/WAL tests and the wide-query
25,000-binding parity case. No final-policy service timing was added.
