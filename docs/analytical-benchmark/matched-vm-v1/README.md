# Matched-VM 10M BI pilot

rdfdb is currently slower than the plain-triples DuckDB baseline on this bounded
BI cohort. Both engines qualify all 60 requests in both passes. Across the 45
measured requests, rdfdb takes 177.023 / 191.764 seconds,
versus DuckDB’s 110.617 / 142.998 seconds.
That is 1.60× / 1.34× as much time.
This is not a qualified four-engine BI ranking: QLever and Virtuoso fail the
prespecified output contract, and Virtuoso’s reverse pass is incomplete.

## Qualified request time

| Engine | First pass, seconds | Reverse pass, seconds |
|---|---:|---:|
| rdfdb | 177.023 | 191.764 |
| DuckDB plain triples | 110.617 | 142.998 |

Each sum covers three weighted 15-query parameter mixes, after one full warmup
mix. Request time runs through complete HTTP body consumption and includes
transport/serialization. JSON parsing, hashing, qualification and QLever cache
clearing are outside the request timer. This is not the official BSBM 50/500-mix
throughput score. Two ordered passes expose sensitivity to run order but do not
establish statistical significance or a general SOTA result.

## Qualification and missing work

The two passes schedule 720 requests. 665 were attempted; 55 are explicitly not
run after Virtuoso’s reverse-pass timeout. No failure is a zero-cost success.
The table includes all 60 scheduled requests per profile/pass, including warmup.

| Profile | First pass | Reverse pass |
|---|---|---|
| rdfdb | 60/60 strict; 60/60 equivalent | 60/60 strict; 60/60 equivalent |
| duckdb | 60/60 strict; 60/60 equivalent | 60/60 strict; 60/60 equivalent |
| virtuoso | 29/60 strict; 29/60 equivalent | 4/5 strict; 4/5 equivalent; 55 not run |
| qlever | 9/60 strict; 21/60 equivalent | 9/60 strict; 21/60 equivalent |
| qlever-hash | 9/60 strict; 21/60 equivalent | 9/60 strict; 21/60 equivalent |
| qlever-bypass | 9/60 strict; 21/60 equivalent | 9/60 strict; 21/60 equivalent |

All QLever configurations have the same status: 9/60 strict passes and 21/60
COUNT-equivalent passes. The remaining 39 equivalent failures are computed-float
datatype failures; the COUNT rule relaxes no float datatype or numeric bound.
Default, explicit hash grouping and full cache bypass remain separate profiles.
Their full-workload totals are unavailable, rather than sums that omit failures.

Virtuoso’s first pass has q4/q6 computed-float datatype failures and wrong q5
result bags in all 12 q5 instances. Its first q5 returns zero rows where Jena,
rdfdb and DuckDB return 14. A compatible-join issue involving q5’s unbound inner
country variable is a hypothesis, not a confirmed reduced diagnosis. The graph
count is correct. The reverse pass times out at 120 seconds on warmup q4 after
four passing requests; 55 later requests are not issued. The container is not
OOM-killed and exits cleanly after the stop. Neither Virtuoso pass has a
qualified full-cohort total.

Strict datatype results and the separately preregistered COUNT-value rule are
reported side by side. Numeric rule freeze:
`a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244`.
COUNT rule freeze:
`ffa49a6ce10c19c02d6185a3431383157c590e1e1beb9c9d37b7b83e03bf5790`.
No original exploratory four-ULP failure has been reclassified.

## Where the qualified engines spend time

Totals below include only the measured mixes, in seconds.

| Template | rdfdb pass 1 | DuckDB pass 1 | rdfdb reverse | DuckDB reverse |
|---|---:|---:|---:|---:|
| BI q1 | 0.711 | 9.202 | 0.726 | 13.424 |
| BI q2 | 77.678 | 6.628 | 89.292 | 8.918 |
| BI q3 | 23.385 | 6.011 | 24.375 | 8.956 |
| BI q4 | 27.641 | 12.190 | 26.963 | 16.752 |
| BI q5 | 46.169 | 22.727 | 48.868 | 27.452 |
| BI q6 | 0.259 | 31.716 | 0.272 | 40.508 |
| BI q7 | 1.011 | 10.525 | 1.078 | 12.630 |
| BI q8 | 0.170 | 11.619 | 0.190 | 14.357 |

BI q2 is an optimization lead: its feature equality is expressed as a FILTER
between separate variables. The pinned executor’s direct ID-equality fast path
covers variable/IRI-constant comparisons, while this variable/variable case uses
numeric compilation/general fallback after the BGP step. Propagating a bound
IRI equality into an index constraint is a candidate to test, with general
literal value equality and scratch-ID fallback preserved. No such change is
included in this benchmark. The earlier untimed stack sample is supporting
diagnosis, not a profile of these timed runs.

## Runtime and resource scope

All engines run serially in the same ARM64 VM with four-CPU quotas and 12 GiB
container memory limits, using fresh owned server processes and stores built
from the same 10,102,032-triple source. Input SHA-256:
`3a207c6725820036eec87d08dd414090d3548ac2839ca192414dad6e352f680e`.
Native rdfdb is pinned to `71ab35cc9e0e512e469b16ad23a56f46d9ee308e` with
wider accumulation, top-k and Linux mimalloc. Copied source, ELF hash and build
image are retained. DuckDB is version 2.0.0; its actual loaded native module is
hashed per phase. QLever/Virtuoso image digests and live settings are retained.

rdfdb’s block cache target is 10,240 MiB. QLever’s query-memory target is 11 GB,
with four query threads and 1 MB cache caps. Cache clearing is independently
verified and repeated before each primary/hash-profile request. Full bypass is
also verified and recorded separately. DuckDB targets 8 GB working memory and
6 GB spill; Virtuoso uses 1,360,000 buffers and four query threads. These settings
have different accounting semantics; they are not equal measured memory use.
The VM is on a shared host; CPU quotas are not dedicated physical-core pinning.

Observed peak RSS below covers the whole server lifetime, including startup,
readiness, warmup and measured requests. Failing/incomplete profiles remain
labelled above; their memory observations are not proof of comparable answers.

| Profile | First pass, GiB | Reverse pass, GiB |
|---|---:|---:|
| rdfdb | 2.136 | 2.311 |
| duckdb | 1.454 | 1.438 |
| virtuoso | 10.302 | 10.274 |
| qlever | 0.353 | 0.349 |
| qlever-hash | 0.314 | 0.314 |
| qlever-bypass | 0.350 | 0.349 |

The 200 ms sampler sees zero bytes of named files under configured TMPDIR and
the DuckDB spill directory. This is not proof of zero spill: unlinked or
short-lived files and engine-specific locations such as Virtuoso’s internal
temporary database are outside that metric. Whole-lifetime RSS and cgroup
memory high-water/limits are retained separately. Configured named temporary
growth has a 5 GiB abort threshold to stay inside the existing scratch allocation.

DuckDB uses one lossless lexical triples table and hand-written translations
for these eight BI templates, with no property tables, materialized joins or
answer cache. This is a corpus-scoped SQL baseline, not a general SPARQL frontend
or proof of the best possible DuckDB schema/SQL formulation.

## Evidence and controller repairs

`round/summary.json` is generated by `bench/analytical/summarize_http.py`, which
checks all scheduled phases, raw-body hashes, eligibility flags and totals.
It refuses to total an unqualified profile. Each phase retains raw bodies,
query-level timings/status, graph-count/readiness checks, runtime configuration,
resources and container exit state. Postchecks preserve the input, binary and
read-only DuckDB file identities. All phase containers exited and the lock was
released after the final postcheck.

Preparation failures are retained: Virtuoso entrypoint configuration, SQL batch
format and an overly short setup timeout caused failed/partial load attempts;
a load-state/count gate rejected the partial store before recovery completed.
During the timing controller, a conservative scratch guard stopped before any
query; DuckDB’s unregistered readiness query, an implicit Docker entrypoint,
and a TIME_WAIT-sensitive port precheck were repaired before any timed request
of the affected profile. Completed phases were preserved, the declared order
resumed, and no timed query was discarded as a repair. Controller versions,
failed setup directories and repair receipts are retained. This does not
eliminate ordinary between-phase timing variation.

`file-manifest.json` records retained runtime files. Larger JSON/body files are
losslessly gzip-compressed with raw/stored hashes. Restore a copy by verifying
the stored hash, decompressing to the manifest’s original path, then verifying
its raw hash/length before running the summary auditor. Operational logs have
credential-related lines sanitized where the manifest marks them; scientific
response bodies remain byte-for-byte unchanged. Service credentials and large
database/binary files are excluded; source/build identities are included.

The host correctness/accuracy cohort is separate in ../holdout-v1/. The 100M
and curated aggregate comparisons remain pending. This pilot does not satisfy
the overall SOTA objective; it identifies concrete correctness and execution
work before that claim can be made.
