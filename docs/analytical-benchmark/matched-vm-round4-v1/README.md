# Same-session three-engine VM BI comparison

rdfdb has the lowest observed total request time in both passes of this bounded
BSBM BI10M cohort. QLever takes **1.42× / 1.24× as long**, and DuckDB
**9.00× / 8.26× as long**, under the separately frozen numeric
value-compatible rule. All 360 scheduled requests pass that rule. QLever still
has strict RDF datatype mismatches; its strict and COUNT-equivalence scores are
shown alongside, without changing their rules.

| Engine | Forward, seconds | Reverse, seconds |
|---|---:|---:|
| rdfdb | 12.567 | 12.987 |
| qlever | 17.787 | 16.166 |
| duckdb | 113.101 | 107.225 |

Each sum covers **45 individual measured HTTP requests** after 15 warmup requests,
not a median, official BSBM 50/500-mix throughput score or statistical significance
study. The forward order is rdfdb → DuckDB → QLever; the reverse order is QLever
→ DuckDB → rdfdb. Every phase starts a fresh owned process. Request timing runs
through complete body consumption; cache clears, parsing, hashes and qualification
are outside the timer. Input graph: **10,102,032 triples**, SHA-256
`3a207c6725820036eec87d08dd414090d3548ac2839ca192414dad6e352f680e`.

## Qualification and ranking scope

| Engine | Strict first / reverse | COUNT-equivalent first / reverse | Value-compatible first / reverse |
|---|---|---|---|
| rdfdb | 60/60 / 60/60 | 60/60 / 60/60 | 60/60 / 60/60 |
| DuckDB | 60/60 / 60/60 | 60/60 / 60/60 | 60/60 / 60/60 |
| QLever default, clear per request | 9/60 / 9/60 | 21/60 / 21/60 | 60/60 / 60/60 |

The numeric-value policy originated in the explicitly retrospective diagnostic
in [value-diagnostic-v1](../value-diagnostic-v1/README.md). For this **new** timing
session its source, workload, references and gate were frozen before any new
response at SHA-256
`70ad4ab0cc3299c20a8177c64568d0698bbe8eed3b391d122214d94ebf1f0f6a`.
The workload was already examined; this is not a blind holdout. The ranking is
conditional on value compatibility, **not strict RDF datatype equivalence**.

The exact declared values of decimal/float/double outputs are compared without
narrowing decimal/double to binary32. Existing rational anchor envelopes apply
to q4/q5/q6; the original four-binary32-step endpoint interval around Jena applies
to q3/q8. COUNT values must be exactly integral and nonnegative. Other RDF term
identities, binding state, row multiplicities, q7 cutoff and outer ordering remain
checked. This is compatibility within bounded numeric error, not exact
mathematical equality. All 60 scheduled requests, including warmup, must pass
before that phase's 45 measured requests can have a value-compatible total.
Strict and COUNT totals/eligibility remain separate in the raw matrix.

Only the default QLever grouping profile was timed here. The hash and bypass
profiles from the older pilot are not pooled into these observations. Virtuoso
was not rerun; its retained q5 bag mismatch and incomplete reverse pass preclude
a qualified total in the old cohort. No new four-engine ranking is claimed.

## What was run

All three engines ran serially in the same ARM64 VM, each with a four-CPU quota
and 12 GiB container memory cap. CPU quotas are not dedicated physical cores;
the shared host and system page cache are uncontrolled. All stores were already
built; startup/load time is outside these request sums. This is a warmed-store
comparison, with QLever's result cache explicitly cleared before every request.

rdfdb corresponds to current main
`cc15f306d5ed9c194b23d6d469e45a89ac5b5fa9`. It reuses the verified Linux binary
actually built at `20b257a239c825d547b2e08ca6bfb69faa8a4c7e` (ELF SHA-256
`b302f11c49cc2e24d6d2a23d1b7cb3c51f2c358f5f17ee49c5395d2cfa3579d4`).
Only README.md differs between those commits; the equivalence and source/binary
metadata are retained. The same binary passed the independent Linux six-row
memo-lifetime fixture before the previous session; it was not rebuilt here.
rdfdb uses four Rayon threads, statistics and a 10,240 MiB block-cache target.

QLever uses image
`adfreiburg/qlever@sha256:d7ecda47b4d0132c190b4a73279761fd1c614f24cba4e894ca1ba8117cef6727`,
four query threads, an 11 GB query-memory target and 1 MB result-cache caps.
Live settings show default hash-map grouping disabled and full cache bypass
disabled. An explicit cache clear before each of the 120 requests returns all
five cache counters to zero; all acknowledgements are replay-checked. This
clears the result cache, not the OS page cache or every internal allocation.

DuckDB is 2.0.0, using four threads, an 8 GB working-memory target and a 6 GB
spill limit. Its loaded native module hash matches both passes and the prior
session. The baseline is one lossless lexical triples table with hand-written
SQL for these eight templates; no property tables, materialized joins or answer
cache. It is corpus-scoped SQL, not a general SPARQL frontend or proof of the
best possible DuckDB layout. Engine memory targets have different accounting
semantics and are not equal measured memory consumption.

## Per-template observations

Each cell sums measured requests of that template, in seconds.

| Template | rdfdb forward | QLever forward | DuckDB forward | rdfdb reverse | QLever reverse | DuckDB reverse |
|---|---:|---:|---:|---:|---:|---:|
| q1 | 0.590 | 0.252 | 9.263 | 0.563 | 0.237 | 8.917 |
| q2 | 0.170 | 2.030 | 6.727 | 0.225 | 1.924 | 6.417 |
| q3 | 2.958 | 0.418 | 5.977 | 2.912 | 0.386 | 5.655 |
| q4 | 6.259 | 10.496 | 12.283 | 6.588 | 9.501 | 11.718 |
| q5 | 1.455 | 1.858 | 22.978 | 1.523 | 1.585 | 21.965 |
| q6 | 0.211 | 1.818 | 32.328 | 0.201 | 1.648 | 30.614 |
| q7 | 0.834 | 0.507 | 11.174 | 0.886 | 0.483 | 10.501 |
| q8 | 0.092 | 0.410 | 12.370 | 0.089 | 0.402 | 11.439 |

QLever has lower observed times on q1, q3 and q7 in both passes, despite rdfdb's
lower overall sum. q3 is a concrete remaining execution/planning lead. These
per-template observations also have only two ordered samples; small differences
should not be treated as stable wins.

## Memory and temporary storage

Observed whole-server high-water RSS includes startup, readiness and warmup:

| Engine | Forward, GiB | Reverse, GiB |
|---|---:|---:|
| rdfdb | 1.371 | 1.376 |
| qlever | 0.349 | 0.349 |
| duckdb | 1.475 | 1.413 |

QLever uses substantially less observed RSS in this session. The configured
cache/query-memory budgets are not the same as actual RSS. A 200 ms sampler
observes zero bytes of named temporary files in the configured directories for
all six phases. This is **not proof of zero spill**: short-lived, unlinked and
other-location files may be missed. Process/cgroup peaks and limits are retained
per phase. All containers stop and are removed; none is OOM-killed. Exit 137
from the controller's explicit stop timeout is retained as an intentional
shutdown, not silently presented as normal exit zero.

## Evidence and validation

[round/summary.json](round/summary.json) contains both original strict/COUNT
eligibility and the separate value-compatible gate/totals. The auditor replays
all value checks from raw bodies, verifies body/reference identities and query metadata,
compares recorded outcomes and totals, and checks every QLever clear.
All 360 requests are attempted and pass the value-compatible rule; no timeout,
missing query or sample discard occurs. All six clients exit zero.

[round/postchecks.json](round/postchecks.json) verifies source NT, read-only
DuckDB file, current/preserved ELF hashes, 26 copied source files, 60 query hashes,
32 numeric bindings, original freezes, live settings and removed containers.
The lock is released only after these checks. No 100M import starts in this window.

The new adapter retains every original strict/COUNT field. Replaying the old
60 QLever bodies checks that preservation and the sum-of-45 gate. Injected wrong
bag, transport failure and not-run cases refuse totals. Fifteen unit tests pass,
including a failed warmup blocking an otherwise successful measured total and
policy-hash rejection. Sources and receipts are retained under benchmark-source/
and validation/. A preflight policy-schema identifier mismatch failed closed
before any HTTP; its corrected schema did not change numeric bounds. The earlier
proposal hash is not the active timing gate, and the failed preflight is retained.

[file-manifest.json](file-manifest.json) binds all 686 retained runtime/setup/
source files by raw hash and length. Large bodies/JSON are losslessly gzip
compressed with stored and raw hashes. Verify the stored hash, decompress to the
original path and verify its raw hash before replaying the auditor. No credentials,
DB blobs or binaries are included. There is no load/build comparison in this run.
The 100M and 72-query aggregate comparisons remain pending, so this result does
not complete the broader analytical SOTA objective.
