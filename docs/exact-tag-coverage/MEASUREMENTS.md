# Statistics-build comparison

Measured baseline **d3e18cd**, candidate **08da299** (baseline plus exact tag coverage only). The same stats_build_probe source was used in both builds. The candidate here excludes task-17 execution tracing. Release, one build job; binary and complete input file hashes are in measurements/receipt.json.

| Snapshot | Baseline warmed median | Coverage warmed median | Ratio |
| --- | ---: | ---: | ---: |
| LUBM-100, 13,409,395 triples | 3.023 s | 3.165 s | 1.047 |
| WatDiv SF100, 10,914,314 triples | 3.444 s | 3.365 s | 0.977 |

Each dataset ran base/new/new/base sequentially. Each process rebuilt statistics three times; the table pools its second and third builds (four warmed observations per configuration, clustered into two processes). All first-build samples are retained. File hashing before measurement warmed the OS cache; no cache was cleared. This is a single session on a shared host, with load averages around 4.7/5.7/6.6 during the window. The RDF benchmark lock serialized our work against the RDF team, but does not prove machine-wide isolation.

LUBM shows a 4.7% increase in this small sample; WatDiv shows a 2.3% decrease. This neither establishes zero overhead nor a stable improvement. Raw per-build times, signatures and receipt are retained. More repetitions would be needed to estimate a small stable effect.

Raw and query-refined estimate float bits and complete candidate-plan hashes match byte-for-byte for all 14 LUBM and 20 WatDiv queries in every process. All snapshot and query file bytes/metadata and both binary hashes were unchanged before/after. This is synopsis/plan parity, not a fresh end-to-end answer comparison.

Functional coverage: full task-17 branch integration on d3e18cd plus coverage passed 93 all-target tests (the API itself previously passed 88 tests under the narrower test invocation). No numeric encoder or range-skip consumer was measured here; those belong to task 11.

Reproduction: run_build_comparison.py requires two fixed binaries, the immutable disk snapshot root, query root and a new output directory. Acquire the shared benchmark lock before building or invoking it. The preserved receipt identifies the actual input contents and paths.
