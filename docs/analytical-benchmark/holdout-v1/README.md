# Frozen 10M BI correctness cohort

All 45 confirmatory BSBM BI query instances pass for Jena 6.2.0, DuckDB 2.0.0,
main rdfdb with ID grouping, the same main binary with general grouping, and the
separate wider-accumulator candidate. All 15 exploratory instances also pass
under this new rule. This is a host correctness run, with no latency ranking.

The numeric rule and exact inputs were frozen before examining mixes 1–3:
`a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244`.
See [the frozen inputs](../numeric-freeze-v1/) for count/magnitude-derived
binary32 error bounds, exact anchors and independent cardinality checks.
Membership, counts, RDF datatypes, bag multiplicities and own-result order
remain exact. q7 uses the preregistered tie-aware cutoff diagnostic.
The original mix-0 four-ULP failures are preserved in real-parameters-v1 and
have not been reclassified as passes under their original contract.

## Runtime identities

- Main: `cb827db96c62ee440539eac31e5d73c771b2ceb1`, extension SHA-256
  `2d30b7b1f84e22b183e59ccb300a2867b3fda49b21ef2b93606f565e89697317`.
  General grouping changes only `RDFDB_LEGACY_GROUP=1`.
- Candidate: `66eaac723d3a5362281260ebbec4f324848bb097`, extension SHA-256
  `1faef24b9b9b3ecffefbc337ae65a180f0c772c69ca2d22767531cb611a907c9`.
  It differs from the pinned main source only in expression/aggregation code
  and its Rust regressions; bindings, dependencies and storage source agree.
- Python 3.14.3, DuckDB 2.0.0, PyArrow 26.0.1; Jena 6.2.0 on the same pinned
  10,102,032-triple graph. Input SHA-256
  `3a207c6725820036eec87d08dd414090d3548ac2839ca192414dad6e352f680e`.

Actual imported native module paths/hashes agree before and after execution.
Every frozen validator source/input hash also agrees afterward. Five supervised
phases exited zero; their complete matrices contain all 60 scheduled queries.
The raw response files, q7 diagnostics, SQL translations, logs, source snapshot,
provenance and postchecks are retained here.

## Confirmatory accuracy

Distances below refer to the correctly rounded exact rational anchor, not to
Jena's output. Relative error instead uses the unrounded exact rational value.

| Aggregate | Main max ULP | Candidate max ULP | Main max relative error | Candidate max relative error |
|---|---:|---:|---:|---:|
| q4 priceRatio | 11 | 1 | 1.248e-6 | 1.129e-7 |
| q5 avgPrice | 3 | 0 | 2.833e-7 | 5.300e-8 |
| q6 reviewerAvgScore | 0 | 0 | 3.815e-8 | 3.815e-8 |

Pointwise absolute error is better/equal/worse for 25/5/0 q4 values,
123/92/0 q5 values and 0/382/0 q6 values. These are observed output values
across the confirmatory instances, including repeated entities/parameters,
not independent statistical samples. There is no observed accuracy regression
in this cohort. This does not prove correct rounding for arbitrary inputs.

Both main grouping modes produce identical full RDF-term bags on all 60
instances. Candidate and DuckDB have the same maxima above; no claim of
bit-identical candidate/DuckDB output follows from those maxima.
Jena is also checked against the exact anchors; it is not treated as an exact
floating-point reference. The machine-readable summary retains its errors too.

## Reading and reproducing the evidence

`file-manifest.json` records every retained runtime file's raw hash/length.
JSON files over 128 KB are losslessly gzip-compressed with both raw and stored
hashes. Restore a copy before running `summarize_holdout.py`: for each manifest
record with `compression=gzip`, verify the stored hash, decompress `stored_path`
to `path`, and verify the raw hash/length. Do not overwrite the archived copy.
The source snapshot's protected files must remain byte-for-byte unchanged.
The summary command accepts the restored directory as `--run` and a new `--out`.
The immutable numeric-input bundle is in the separate numeric-freeze-v1 tree.

A one-second stack sample from main/general landed in RDF filter evaluation
and dictionary resolution. It is retained as diagnosis, not a query latency or
whole-run profile. Correctness-phase wall time includes validation and setup;
it is not a competitor speed measurement. Matched-VM HTTP integration, actual
cache-control verification, memory/spill observation and the 100M track remain
pending. The eight HTTP/adapter unit checks qualify only those mechanisms.
