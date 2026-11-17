# Analytical RDF benchmark: initial qualification

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

Next: requalify the float-corrected engine; validate actual q7 cutoff ties and
error/empty aggregate fixtures; build the 10M DuckDB baseline; then run matched
Linux/VM competitor measurements. The 100M track follows disk/memory sizing.
No unsupported query, timeout or failed result is removed from a full BI score.
