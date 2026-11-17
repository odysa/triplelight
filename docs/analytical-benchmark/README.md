# Analytical RDF benchmark: qualification in progress

There is no full BI performance score yet. The corrected candidate passes the
normal, cutoff, invalid-input and empty/unbound fixtures against Jena. Large-data
qualification, matched competitor execution and resource measurement remain ahead.

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

Next: build the 10M DuckDB baseline and qualify real parameter instances; then run matched
Linux/VM competitor measurements. The 100M track follows disk/memory sizing.
No unsupported query, timeout or failed result is removed from a full BI score.
