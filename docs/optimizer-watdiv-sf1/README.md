# WatDiv SF1 cardinality validation

The [official WatDiv v0.6](https://dsg.uwaterloo.ca/watdiv/) model generated
116,275 statements, 110,680 unique triples, 5,597 subjects and 798 characteristic
sets. Generator seeds are fixed at 20261112. One instance of each of the 20
basic templates is retained, including empty results. Source, data and query
checksums are in `provenance.json`; exact generation steps are in `GENERATE.md`.
Data and dependencies are outside Git.

Independent Jena 6.2.0 ARQ processes counted each saved SELECT's bag of bindings
using a COUNT over a nested SELECT. `reference_counts.py` reproduces that method.
No inference or query modifiers are used. The Rust q-error example verified that
all 20 queries have those same counts, and that greedy and cost-ordered index
nested-loop execution produce identical complete RDF-term result multisets.
The check disables leapfrog so it actually tests the optimizer's index-loop order.

This is a small-scale estimator check, not the WatDiv-10M benchmark. Twelve of
twenty queries return zero. Eight have positive counts: their median q-error is
2.714286 and maximum 24 (L1). Four empty queries have positive estimates (C2, F1,
L5, S4), giving **infinite q-error** under the stated zero convention. No positive
answer has an estimated zero. `qerror.tsv` includes every case; `summary.json`
separates positive and empty cases rather than hiding infinity or treating many
empty results as strong accuracy evidence.

The estimates remain limited by uniform chain selectivity and within-CS degree
and object correlations. LUBM Q9's 251-fold underestimate remains unresolved.
This fixture gives correctness and error measurements, no latency or SOTA claim.
Larger scales and multiple query instances remain necessary performance work.
