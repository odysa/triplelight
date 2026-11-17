# Analytical RDF benchmark: qualification in progress

Current result: all 45 confirmatory 10M BI instances pass for Jena, DuckDB,
both main rdfdb grouping paths and the separate wider-accumulator candidate
under the preregistered exact-anchor rule. The candidate improves q4/q5
accuracy; the original exploratory failures remain preserved. See
[holdout-v1/](holdout-v1/) for complete outputs, immutable identities and
accuracy details. This is correctness evidence; matched-VM BI performance and
100M measurements remain pending.


There is no full BI performance score yet. The corrected candidate passes the
normal, cutoff, invalid-input and empty/unbound fixtures against Jena. Matched competitor execution and resource measurement remain ahead.

## Matched-VM preparation

[vm-preflight-v1/](vm-preflight-v1/) pins the ARM64 competitor images and records
live QLever cache/settings checks. The 1 MB caps allow a cached small aggregate
answer; explicit clearing and full bypass are verified separately on a tiny
fixture. These checks do not establish cache hits in historical benchmark runs.

[count-equivalence-v1/](count-equivalence-v1/) preregisters an optional, separately
reported COUNT int/integer value-equivalence rule before QLever confirmatory BI
outputs. Strict datatype failures remain visible. Neither policy changes the
already frozen host cohort or supplies a competitor latency score.

## Numeric validator frozen before the confirmatory cohort

The exact-anchor policy is now frozen before mix 1–3 candidate outputs, under
SHA-256 `a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244`. All 19 tests pass, including arithmetic bounds,
datatype/entity/count rejection, tamper detection and retaining failed reference
outputs. Independent Jena counts agree on all 31 parameter cases and 22,381 input
entity groups. Exact rational inputs cover all 32 AVG-derived instances.

See [numeric-freeze-v1/](numeric-freeze-v1/) and the
[validation rule](NUMERIC_VALIDATION_PROPOSAL.md). This enables a named future
cohort; it does not reclassify mix 0 or supply BI speed numbers. Current
workload_check.py activates it only with an explicit matching input bundle and
freeze hash; its default remains the original four-ULP checker.

## First real-parameter cohort: partial, numeric gate not satisfied

Fifteen official parameter instances (mix 0) completed serially on Jena,
DuckDB and both rdfdb aggregation paths. The candidate engine is merged main
cb827db96c62ee440539eac31e5d73c771b2ceb1, with one pinned Python extension for
both paths. `RDFDB_LEGACY_GROUP=1` selects the general path; the other process
unsets it. Source-data and extension hashes are unchanged after execution.

| Candidate vs Jena, original four-ULP gate | Pass | Fail | q4 maximum ULP / relative difference |
|---|---:|---:|---|
| DuckDB plain triples | 14 | 1 | 7 / 7.68e-7 |
| rdfdb ID aggregation | 14 | 1 | 27 / 2.93e-6 |
| rdfdb general aggregation | 14 | 1 | 27 / 2.93e-6 |

The q4 failure is numeric: all three return the same ordered ten feature IRIs
and each candidate's own numeric ordering is monotonic. Both rdfdb paths return
identical complete RDF term bags on all fifteen instances. This isolates a
validation question; it does not prove that either Jena or a candidate is wrong.
Different accumulation orders/precision are a hypothesis requiring independent
input and arithmetic checks. The original failed gate is retained unchanged.
See the [future numeric-validation proposal](NUMERIC_VALIDATION_PROPOSAL.md).

This is only the first 15 of 60 exported queries, with no measured BI timing or
competitor ranking. The reference process exited before candidate work;
candidate processes also ran serially. All workers and the Jena service exited,
and the shared lock was released. The 8-template small-graph smoke of this
serial runner passed against actual Jena HTTP, DuckDB and rdfdb.

Large JSON outputs are stored losslessly as `.json.gz`;
`real-parameters-v1/compressed_artifacts.json` records raw and compressed hashes.
Restore the original bytes with `gzip -dc file.json.gz > file.json` in a copy.

[real-parameters-v1/](real-parameters-v1/) contains complete outputs, SQL,
matrices, numeric diagnostics, source/runtime identities and process logs.
[workload-check-smoke/](workload-check-smoke/) retains the earlier actual-engine
runner validation. The 10M cohort uses its own earlier source snapshot; newer
spill configuration in the current runner is not retroactively attributed to it.
A separate [bounded-spill smoke](workload-check-spill-smoke/) passes the eight
small-graph DuckDB queries with a 256 MB query-spill setting.

## 10M DuckDB baseline setup

The pinned BSBM-10M input imported successfully: **10,102,032 raw and unique
triples**, SHA-256 3a207c6725820036eec87d08dd414090d3548ac2839ca192414dad6e352f680e.
The plain-triples database is 619,196,416 bytes. This one host setup run used
DuckDB 2.0.0/PyArrow 26.0.1, four threads, an 8 GB DuckDB memory setting and an
8 GB spill setting. Observed whole-process peak RSS was 4,487,774,208 bytes;
spill file lengths sampled every 200 ms peaked at zero, with no sampling errors.
Those settings are not an RSS hard limit, and sampling can miss brief spill.

Setup took 201.85 seconds, including the Python N-Triples parser, Arrow conversion,
graph deduplication and checkpoint. This is neither a native DuckDB loader
comparison nor a same-session competitor result. [import-v1/](import-v1/) retains
source, invocation, input identities and resource evidence. A small metrics probe
first confirmed 84 raw statements became 83 unique triples. Its first launcher
attempt incorrectly resolved the virtual-environment executable to the global
Python and failed before import; that failed log is retained separately.

## Current candidate: fast-group d2618c6

The aggregate-error correction passes the extended fixture suite. An isolated
Python extension was rebuilt from exactly
d2618c6c9c6d51541d9298e8db88f131f5ab2eb6; its SHA-256 is
207faf4b8250f63ee2d01e891cbf57257a940da0dc9db596b2c86019b9154024.
Its extension and package hashes remain unchanged after execution. Jena 6.2.0
is the independent reference; DuckDB is 2.0.0.

| Fixture | Requested BI templates | DuckDB vs Jena | rdfdb vs Jena |
|---|---|---|---|
| Normal 83-triple graph | q1–q8 | All exact passes | All exact passes |
| Actual top-1,000 cutoff ties | q7 | Allowed-result pass | Allowed-result pass |
| Invalid USD price | q4, q5, q8 | All exact passes | All exact passes |
| Invalid typed rating | q6 | Exact pass | Exact pass |
| Empty graph | q1–q8 | All exact passes | All exact passes |

Four separate aggregate diagnostics also match Jena exactly: mixed bound/unbound
values, all unbound values, an empty implicit group and an empty explicit group.
Each projects COUNT-star, COUNT, COUNT DISTINCT, SUM, AVG, MIN, MAX, SAMPLE and
GROUP_CONCAT. These diagnostics compare rdfdb with Jena only; they are neither
additional official BI templates nor a DuckDB translation claim. Five existing
oracle tests pass. All checks are untimed correctness checks.

[qualification-v3/](qualification-v3/) retains the graphs, requested queries,
complete outputs, matrices, source snapshots, build/source identities and logs.
This is bounded qualification, not full SPARQL numeric conformance or large-data
qualification. The old failing outputs remain unchanged below.

The workload directory also preserves 72 curated LUBM/WatDiv aggregate queries
from the earlier count-pushdown cohort, verified byte-for-byte against ac101b3.
They are new benchmark inputs here; historical engine A/B timings are not results
for the current four-system analytical comparison.

## Previous candidate: fast-group 63d54f7

An isolated Python extension was rebuilt from exactly
63d54f75168ebac7cc4c52e536fcfc00809cc53c. Its SHA-256 is
976df9b4049a1b26f579046049cf73e193bfc382d755d9237b69b9e450e01664.
The package path was checked to prevent the older editable binding shadowing it.
The post-run hash is unchanged. All results use Jena 6.2.0 as the independent
reference and DuckDB 2.0.0 for the typed plain-triples translation.

| Fixture | Requested BI templates | DuckDB vs Jena | rdfdb vs Jena |
|---|---|---|---|
| Normal 83-triple graph | q1–q8 | All exact passes | All exact passes |
| Actual top-1,000 cutoff ties | q7 | Allowed result, 500 rows | Allowed result, 499 rows |
| Invalid USD price | q4, q5, q8 | All exact passes | Fail: bad aggregate input ignored |
| Invalid typed rating | q6 | Exact empty result | Fail: three reviewers returned |

The normal graph deliberately exposes unbound-country compatibility in q5
and duplicate-score UNION/global-average scope in q6. All normal outputs match
complete RDF term bags exactly, including computed numeric datatype and lexical
form; no ULP allowance was needed in that fixture. This is bounded fixture
evidence, not a proof of arbitrary-precision or all-SPARQL correctness.

The cutoff graph contains 1,002 products tied for 997 inner slots. The outer
NOT EXISTS filter can then leave 497–502 rows. Jena returns 499, DuckDB 500 and
rdfdb 499; all are valid. The validator first compares an untimed all-product
count/blocked diagnostic against independent Jena, then checks mandatory members,
permitted boundary members, duplicate absence and cardinality bounds. It does not
append a tie-breaker to the official query or demand an incorrectly exact bag.

The invalid-price graph adds one `"bad"` USD literal. Jena leaves the affected
q4 ratios and q5 average unbound and removes one q8 vendor. rdfdb instead
returns the original numbers/vendor. The invalid-rating graph adds an ill-typed
integer literal; Jena's q6 is empty while rdfdb returns three reviewers. DuckDB's
guarded aggregates agree with Jena in all four requested cases. These are
preserved failures, not qualified timing samples.

[qualification-v2/](qualification-v2/) contains all source graphs, queries,
SQL, reference/candidate results, matrices, build/source hashes and logs.
The first failure matrix remains unchanged below and in qualification-v1/.
The candidate is not merged by this benchmark task; engine changes belong to
the engine owner.

## Earlier qualification: before the float fix

Status: active, no full BI performance score yet. The initial small-graph
qualification compares Jena 6.2.0, DuckDB 2.0.0 and rdfdb binding 5b3970b.
This is a correctness/support check, not a timing round.

| BI template | DuckDB vs Jena | rdfdb vs Jena |
|---|---|---|
| q1 | Exact pass | Exact pass |
| q2 | Exact pass | Exact pass |
| q3 | Exact pass | Fail: double instead of float |
| q4 | Exact pass | Fail: double instead of float |
| q5 | Exact pass | Fail: double instead of float |
| q6 | Exact pass | Fail: double instead of float |
| q7 | Exact pass, small graph without cutoff ambiguity | Exact pass, small graph without cutoff ambiguity |
| q8 | Exact pass | Fail: double instead of float |

All eight SQL results match complete Jena RDF term bags, including computed
values, with zero ULP deviation in this deliberately exact-valued fixture.
This does not establish correctness at arbitrary scale or on all RDF data.
The five rdfdb failures are datatype failures even though the displayed
values match on this fixture; they are not counted as passing values.

The graph contains 84 raw statements and 83 unique triples. It exposes q5's
unbound-country compatibility and q6's duplicate-score UNION bag and the
difference between global-average and reviewer-bound scopes. Every reference
and candidate output, SQL text, query and source graph is retained in
[qualification-v1/](qualification-v1/). Runtime wheel hashes and import graph
counts are included. The pinned rdfdb extension SHA-256 is
898940920addbc19743b4b06714787d0766415e55ffa34790b91da1c61de05e1.

The official parameter-pool exporter has generated 60 BSBM-10M BI instances
(seed 808080, four weighted 15-query mixes). These use one warmup and three
measured mixes rather than the official tool defaults of 50/500. They are
workload inputs only, not 60 successful large-data query measurements.
See bench/analytical/workloads/bsbm10/ and the measurement contract in
bench/analytical/README.md.

Next: qualify real parameter instances against the imported 10M graph; then run matched
Linux/VM competitor measurements. The 100M track follows disk/memory sizing.
No unsupported query, timeout or failed result is removed from a full BI score.
