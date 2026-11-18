# Round 6: aggregate lead, q1 gap and variable q7 outcome

rdfdb has the lowest observed aggregate request time in both passes on main
`5ac99cc`, under the unchanged numeric value-compatible rule. All 360 scheduled
requests pass that rule. **q1 has not flipped against QLever; q7 is effectively
tied forward and lower for rdfdb in reverse.** The large within-session variation
precludes calling that a stable q7 flip. q3 remains slower than QLever.

| Engine | Forward, seconds | Reverse, seconds |
|---|---:|---:|
| rdfdb | 10.788 | 7.275 |
| qlever | 18.278 | 18.659 |
| duckdb | 170.715 | 113.401 |

QLever takes **1.69× / 2.56× as long** as rdfdb; DuckDB takes
**15.82× / 15.59× as long**. Each sum covers 45 individual measured
HTTP requests after 15 warmup requests. This is a bounded BSBM BI10M cohort, not
the official 50/500-mix throughput score, significance study or general SOTA proof.

## Per-template observations and variation

Measured-mix sums, in seconds:

| Template | rdfdb forward | QLever forward | DuckDB forward | rdfdb reverse | QLever reverse | DuckDB reverse |
|---|---:|---:|---:|---:|---:|---:|
| q1 | 0.481 | 0.260 | 15.819 | 0.314 | 0.278 | 9.777 |
| q2 | 0.170 | 2.055 | 10.448 | 0.105 | 2.165 | 6.747 |
| q3 | 1.298 | 0.423 | 10.323 | 0.971 | 0.436 | 6.119 |
| q4 | 5.714 | 10.895 | 18.303 | 4.057 | 11.005 | 12.357 |
| q5 | 2.066 | 1.779 | 35.618 | 1.199 | 1.903 | 23.141 |
| q6 | 0.357 | 1.879 | 55.069 | 0.214 | 1.913 | 32.205 |
| q7 | 0.534 | 0.542 | 12.574 | 0.316 | 0.540 | 11.062 |
| q8 | 0.169 | 0.444 | 12.562 | 0.099 | 0.418 | 11.993 |

- q1: rdfdb **481 / 314 ms**, QLever **260 / 278 ms**; QLever is lower in both.
- q3: rdfdb **1,298 / 971 ms**, QLever **423 / 436 ms**; the gap remains.
- q7: rdfdb **534 / 316 ms**, QLever **542 / 540 ms**; the forward difference
  is about 1.5%, effectively a tie at this observation scale. The reverse pass
  has a larger rdfdb advantage, but two ordered passes do not establish stability.

rdfdb's aggregate varies from 10.788 to 7.275 s, and DuckDB's from 170.715 to
113.401, while QLever is steadier at 18.278 / 18.659. Both passes are retained;
none is discarded or selected as the preferred result. The shared host and OS
cache are uncontrolled, and the cause of this variation is not established.
Do not interpret these data as a controlled old/new-binary A/B or infer that the
host optimization gains transfer directly to the VM. The previous
[round 5](../matched-vm-round5-v1/README.md) is a separate session, not additional
samples pooled into the ratios above.

## Qualification and prospective timing gate

| Engine | Strict first / reverse | COUNT-equivalent first / reverse | Value-compatible first / reverse |
|---|---|---|---|
| rdfdb | 60/60 / 60/60 | 60/60 / 60/60 | 60/60 / 60/60 |
| DuckDB | 60/60 / 60/60 | 60/60 / 60/60 | 60/60 / 60/60 |
| QLever default, clear per request | 9/60 / 9/60 | 21/60 / 21/60 | 60/60 / 60/60 |

The ranking is conditional on numeric value compatibility, **not strict RDF
datatype equivalence**. The unchanged policy SHA-256 is
`70ad4ab0cc3299c20a8177c64568d0698bbe8eed3b391d122214d94ebf1f0f6a`.
It binds validator sources, references and workload before new responses. Its
origin was an explicitly [retrospective diagnostic](../value-diagnostic-v1/README.md)
on already-seen outputs; this cohort is not a blind holdout. Original strict and
COUNT outcomes remain separate and unchanged.

Exact declared decimal/float/double values are compared without narrowing decimal
or double to binary32. Original rational anchor envelopes apply to q4/q5/q6;
q3/q8 use the four-binary32-step endpoint interval around Jena. COUNT must be
exactly integral and nonnegative. Other RDF identities, binding state, complete
row multiplicities, q7 cutoff and ordering remain checked. This is compatibility
within existing numeric bounds, not exact mathematical equality. Every scheduled
request, including warmup, must pass before its 45 measured-request phase total
is eligible. No timeout, missing query, failed value response or sample discard
occurs in this session.

Only QLever's default grouping/cache-clear profile is timed. The old hash/bypass
profiles are not pooled; Virtuoso is not rerun. No four-engine ranking is made.

## Source and runtime

rdfdb is freshly built from main
`5ac99ccbe1a008a381923300920f426fcc766e2a`, including the independently qualified
[batched EXISTS](../exists-batch-qualification-v1/README.md) and
[hash extension](../hash-extend-qualification-v1/README.md) changes since round 5.
The 26 copied source files are checksum-verified inside the pinned Rust builder
**after copying into /b**. Source/build command/log, manifest and executable
binding are retained. Linux executable SHA-256:
`5545f33a2ded6ae9f1dce4d8fa8b98d4de18dffdf3c86116b9e77b8dd4d5e637`.
The new executable passes the six-row independent lifetime fixture before timing.
Earlier 1cd/20b/738/71 executables remain preserved and hash-verified afterward.

All engines run serially in the same ARM64 VM with four-CPU quotas and 12 GiB
container caps, fresh processes and prebuilt warmed stores. Forward order is
rdfdb → DuckDB → QLever; reverse is QLever → DuckDB → rdfdb. CPU quotas are not
dedicated physical cores. Startup/load/build time is excluded from these request
sums; no load/build comparison is made. The input is 10,102,032 triples, SHA-256
`3a207c6725820036eec87d08dd414090d3548ac2839ca192414dad6e352f680e`.

rdfdb uses four Rayon threads, statistics and a 10,240 MiB cache target. QLever
uses four query threads, an 11 GB query-memory target, 1 MB result-cache caps and
image `adfreiburg/qlever@sha256:d7ecda47b4d0132c190b4a73279761fd1c614f24cba4e894ca1ba8117cef6727`.
Default hash grouping and full bypass are disabled. All 120 explicit request-cache
clears have all five counters at zero and are replay-verified; this does not clear
the OS page cache. DuckDB 2.0.0 uses four threads, 8 GB work memory, 6 GB spill and
the same read-only lexical triples baseline with scoped SQL translations. Its
actual native module identity is checked in both passes.

Timing runs through complete HTTP body consumption. Cache clears, JSON parsing,
hashes and qualification are outside the request timer. Full commands, live
settings and cache acknowledgements are retained.

Observed whole-server high-water RSS, including startup/readiness/warmup:

| Engine | Forward, GiB | Reverse, GiB |
|---|---:|---:|
| rdfdb | 1.414 | 1.534 |
| qlever | 0.352 | 0.349 |
| duckdb | 1.447 | 1.442 |

QLever retains a substantial observed RSS advantage. Configured budgets are not
actual RSS. The 200 ms named-file samplers see zero temporary bytes in all phases;
this is not proof of zero spill because transient, unlinked or other-location
files may be missed. Process/cgroup peaks and limits remain in the raw evidence.
All six clients exit zero, all containers are explicitly stopped/removed and
none is OOM-killed. Controller stop-timeout exit 137 is retained as intentional
shutdown, not rewritten to exit zero.

## Audit and retained evidence

[round/summary.json](round/summary.json) retains original strict/COUNT eligibility
and independently replays value coverage and totals. Every raw body/reference
identity, query metadata and QLever clear is verified. Both Linux rdfdb passes
also match the qualified host `5ac99cc` outputs on all 60 exact bags and all
36 ordered sequences; this is an extra check, not a changed timing gate.
[round/postchecks.json](round/postchecks.json) records source NT, read-only DuckDB
file, old/new ELF, 26 source, 60 query, 32 numerical-binding and original-freeze
checks, fixture/host equivalence and removed containers. The shared lock is
released only after all checks finish.

The value validator remains the same source-verified version whose 15 unit tests
and adapter positive/negative gates passed in
[round 4](../matched-vm-round4-v1/validation/); no policy or controller repair is
made during this run. [file-manifest.json](file-manifest.json) binds all 716 retained
runtime/setup/source files. Larger JSON/body files are losslessly gzip-compressed
with stored and raw hashes. Verify stored bytes, decompress to the original path,
then verify raw hash/length before replay. No credential, executable or database
blob is included.

The 100M and 72 curated aggregate comparisons remain pending. This observation
does not complete the broader analytical SOTA objective.
