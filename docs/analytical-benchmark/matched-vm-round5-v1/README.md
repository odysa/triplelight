# Three-engine VM rerun on main 1cd8881

rdfdb has the lowest observed total request time in both passes of this bounded
BSBM BI10M cohort. QLever takes **2.05× / 1.61× as long**, and DuckDB
**12.53× / 11.13× as long**, under the same separately frozen
numeric value-compatible rule used in round 4. All 360 scheduled requests pass
that rule. **The q1, q3 and q7 gaps against QLever are not closed in this session**:
QLever has lower observed sums on each of those templates in both passes.

| Engine | Forward, seconds | Reverse, seconds |
|---|---:|---:|
| rdfdb | 9.428 | 10.626 |
| qlever | 19.355 | 17.080 |
| duckdb | 118.179 | 118.303 |

Each sum covers **45 individual measured HTTP requests** after 15 warmup requests,
not a median, official BSBM 50/500-mix throughput score or a significance study.
Forward order is rdfdb → DuckDB → QLever; reverse is QLever → DuckDB → rdfdb.
Fresh owned processes run serially. Request time includes complete HTTP body
consumption; cache clears, parsing, hashes and qualification are outside it.
The input has **10,102,032 triples**, SHA-256
`3a207c6725820036eec87d08dd414090d3548ac2839ca192414dad6e352f680e`.

## Qualification and rule scope

| Engine | Strict first / reverse | COUNT-equivalent first / reverse | Value-compatible first / reverse |
|---|---|---|---|
| rdfdb | 60/60 / 60/60 | 60/60 / 60/60 | 60/60 / 60/60 |
| DuckDB | 60/60 / 60/60 | 60/60 / 60/60 | 60/60 / 60/60 |
| QLever default, clear per request | 9/60 / 9/60 | 21/60 / 21/60 | 60/60 / 60/60 |

The numeric-value policy is unchanged from round 4, SHA-256
`70ad4ab0cc3299c20a8177c64568d0698bbe8eed3b391d122214d94ebf1f0f6a`.
Its source, references and cohort were bound before this session. Its origin was
an explicitly retrospective [value diagnostic](../value-diagnostic-v1/README.md),
and the cohort was already seen; this is not a blind holdout. The ranking is
conditional on value compatibility, **not strict RDF datatype equivalence**.
No strict/COUNT score or earlier failure is reclassified.

Exact declared decimal/float/double values are compared without narrowing decimal
or double outputs to float. Original rational anchor envelopes apply to q4/q5/q6,
and the four-binary32-step endpoint interval around Jena applies to q3/q8. COUNT
values must be exactly integral and nonnegative. Other RDF identities, binding
state, row multiplicities, q7 cutoff and outer ordering remain checked. This is
numeric compatibility within existing bounds, not exact mathematical equality.
All 60 scheduled requests, including warmup, must pass before that phase's
45 measured requests have a value-compatible total. Failed/missing requests
would not become zero-cost successes. There are none in this run.

Only QLever's default grouping/cache-clear profile is timed here. The old hash
and bypass profiles are not pooled. Virtuoso is not rerun and has no qualified
total from the old incomplete/mismatching cohort. No four-engine ranking is made.

## Source and build

rdfdb is pinned to main
`1cd888103dfdb93643ed158fe6d077e90da27b4b`, containing the independently qualified
[filter memo](../filter-memo-qualification-v1/README.md),
[intern cache](../intern-cache-qualification-v1/README.md) and
[membership set](../probe-set-qualification-v1/README.md) changes. The parked
merge-join WIP is excluded. The Linux executable is freshly built from 26 copied
source files, checked by SHA-256 **inside the container after copying into /b**.
The source, manifest, pinned Rust build image, command/log and binary binding are
retained. ELF SHA-256:
`f91a64faf85142685ca1fc59f1244f3c82d6ab9e38d5b532a55a001957c59cb4`.
Earlier 20b/738/71 executables remain preserved and postverified.

Before timing, this new Linux executable passes the independent six-row
memo-lifetime fixture exactly against Jena. After timing, each new Linux rdfdb
pass matches all **60 complete host 1cd8881 bags and 36 ordered sequences**.
Those host reference bodies are verified against their published archive hashes.
These extra host/VM checks do not alter the frozen timing gate.

## Runtime and observed resources

All three engines run in the same ARM64 VM with four-CPU quotas and 12 GiB caps.
CPU quotas are not dedicated physical cores; the shared host and system page
cache are uncontrolled. Stores are prebuilt and warmed; startup/load/build time
is outside these request sums. No load/build comparison is made.

rdfdb uses four Rayon threads, statistics and a 10,240 MiB block-cache target.
QLever uses four query threads, an 11 GB query-memory target, 1 MB result-cache
caps and image
`adfreiburg/qlever@sha256:d7ecda47b4d0132c190b4a73279761fd1c614f24cba4e894ca1ba8117cef6727`.
Its hash-map grouping and full bypass remain disabled. Explicit result-cache
clears before every request have all five counters at zero; all 120 request
acknowledgements verify. This does not clear the OS page cache. DuckDB 2.0.0 uses
four threads, an 8 GB work-memory target, 6 GB spill limit and the same lossless
lexical triples file and template-specific SQL baseline as before. Native module
identity is checked in both passes. Full commands/live settings are retained.

Observed whole-server high-water RSS includes startup/readiness/warmup:

| Engine | Forward, GiB | Reverse, GiB |
|---|---:|---:|
| rdfdb | 1.349 | 1.369 |
| qlever | 0.353 | 0.353 |
| duckdb | 1.470 | 1.414 |

QLever retains a substantial observed RSS advantage. Configured budgets are not
actual RSS. All six 200 ms named-file samplers see zero temporary bytes; this is
not proof of zero spill, because unlinked, transient or other-location files may
be missed. Process/cgroup peaks and limits are retained. All six clients exit
zero, all containers are explicitly stopped/removed and none is OOM-killed.
Controller stop-timeout exit 137 is retained as intentional shutdown, not exit zero.

## Template observations and prior session

These are sums for the measured mixes, in seconds:

| Template | rdfdb forward | QLever forward | DuckDB forward | rdfdb reverse | QLever reverse | DuckDB reverse |
|---|---:|---:|---:|---:|---:|---:|
| q1 | 0.321 | 0.275 | 10.216 | 0.360 | 0.246 | 10.102 |
| q2 | 0.093 | 2.310 | 6.996 | 0.094 | 2.055 | 7.177 |
| q3 | 0.740 | 0.458 | 6.323 | 0.827 | 0.402 | 6.249 |
| q4 | 6.434 | 11.437 | 12.403 | 7.168 | 10.056 | 13.165 |
| q5 | 0.955 | 1.892 | 23.691 | 1.140 | 1.695 | 23.919 |
| q6 | 0.196 | 1.943 | 33.774 | 0.196 | 1.740 | 33.533 |
| q7 | 0.614 | 0.564 | 11.765 | 0.745 | 0.492 | 11.465 |
| q8 | 0.075 | 0.475 | 13.011 | 0.094 | 0.394 | 12.694 |

For q1, rdfdb takes 321 / 360 ms against QLever's 275 / 246; for q3, 740 / 827 ms
against 458 / 402; for q7, 614 / 745 ms against 564 / 492. These gaps remain
optimization leads. In particular, the small q7 forward difference should not
be treated as statistically resolved by two passes.

The previous [round 4](../matched-vm-round4-v1/README.md) used an earlier rdfdb
binary and recorded rdfdb totals 12.567 / 12.987 s, QLever 17.787 / 16.166 and
DuckDB 113.101 / 107.225. q3's earlier rdfdb template sums were 2.958 / 2.912 s.
The new session is consistent with substantial q3 progress, but it is not a
controlled old/new-binary A/B: competitors also vary between sessions. Attribute
only the paired, same-session ratios above directly to this comparison. No
cross-session pooling, significance claim or general SOTA claim is made.

## Audit and evidence

[round/summary.json](round/summary.json) preserves original strict/COUNT outcomes
and separately audits value-compatible coverage/totals. The unchanged auditor
replays values from raw responses, verifies reference/body/query metadata,
compares recorded outcomes and totals, and checks all QLever clear replies.
[round/postchecks.json](round/postchecks.json) records source/input/read-only
DuckDB file/current and old ELF/26 source/60 query/32 numeric-binding identities,
original freezes, the new Linux fixture, host-equivalence counts and removed
containers. All checks pass before the shared lock is released.

The value gate's 15 unit tests and positive/negative adapter replay receipts were
already validated in round 4; its sources are unchanged and hash-verified here.
They are retained in the [prior validation archive](../matched-vm-round4-v1/validation/).
No new controller repair, missing query, timeout, sample discard or policy change
occurs in this session. [file-manifest.json](file-manifest.json) binds all
716 retained runtime/setup/source files. Larger JSON/body files use lossless gzip
with stored and raw hashes. Verify stored bytes, decompress to the original path
and verify raw hashes/lengths before replay. No credentials, executable or
database blobs are included.

The 100M and 72 curated aggregate comparisons remain pending. This result does
not complete the broader analytical SOTA objective.
