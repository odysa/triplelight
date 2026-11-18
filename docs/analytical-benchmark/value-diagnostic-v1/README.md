# Retrospective numeric value comparison of the retained VM pilot

QLever's retained results pass all 60 requests in each pass when designated
computed numeric outputs are compared by value within the original bounds,
regardless of numeric datatype. This holds for its default, hash-grouping and
cache-bypass profiles. Virtuoso passes 48/60 first-pass requests under that
value-only diagnostic; all 12 q5 instances still fail the entity/count bag check.
The strict and preregistered COUNT-equivalence scores remain unchanged.

| Retained profile | Strict first / reverse | COUNT-equivalent first / reverse | Retrospective value-only first / reverse |
|---|---|---|---|
| rdfdb (`71ab35c`, original pilot) | 60/60 / 60/60 | 60/60 / 60/60 | 60/60 / 60/60 |
| DuckDB | 60/60 / 60/60 | 60/60 / 60/60 | 60/60 / 60/60 |
| QLever default | 9/60 / 9/60 | 21/60 / 21/60 | **60/60 / 60/60** |
| QLever hash grouping | 9/60 / 9/60 | 21/60 / 21/60 | **60/60 / 60/60** |
| QLever cache bypass | 9/60 / 9/60 | 21/60 / 21/60 | **60/60 / 60/60** |
| Virtuoso | 29/60 / incomplete | 29/60 / incomplete | **48/60 / incomplete** |

Virtuoso's reverse pass has four passing requests, one q4 client timeout and 55
requests not run, under all three checks. The denominator above includes warmup:
60 scheduled requests per profile/pass, with 15 warmup and 45 measured. This is
the old four-engine pilot, not a rerun of current rdfdb or the new two-engine
round3. Its runtime configurations and source/image pins remain those in
[matched-vm-v1](../matched-vm-v1/README.md). No timing total, retrospective speed
ranking, or preregistered qualification is created by this diagnostic.

## What the new column checks

This policy was written after competitor outputs had been examined and at the
reviewer's request. It is explicitly **retrospective**. “Value-only” means numeric
compatibility within existing bounds, not exact mathematical equality or RDF
term identity. Only the designated computed numeric and COUNT columns lose
numeric datatype identity. Other RDF term identities, result multiplicities,
binding state, q7 cutoff rules and outer ordering remain checked. All reported
value passes also pass their applicable outer-order check.

For q4/q5/q6, exact rational output values must lie in the original independently
verified count/magnitude anchor envelopes. For q3/q8, the interval endpoints are
four binary32 representable steps on either side of Jena's float value, retaining
the old four-step bound. Decimal values are exact rationals; float and double
values are decoded as their declared binary32 and binary64 value spaces. Decimal
and double outputs are **not rounded down to float** to make a bound pass. COUNT
values must be exactly integral and nonnegative. Invalid literals and unsupported
non-finite values fail. See [POLICY.md](POLICY.md) and
[policy-source-freeze.json](policy-source-freeze.json).

The original numeric freeze remains
`a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244`.
No frozen validator, input, prior status or raw response was edited. Each
attempted complete raw body is checked against both its original measurement
hash and the published archive manifest; Jena reference files and query text are
also checked against their published manifest/hash. The original first-failure
labels alone could not prove other values correct; this separate re-score now
checks the previously rejected numeric values explicitly.

## Virtuoso q5 evidence

[query.rq](virtuoso-q5/query.rq) is the exact first q5 request,
`m000_p05_q5.rq`, SHA-256
`35a2fc791f57aca1cbf7d84ddcd136a57849c619f73d1d3bfb3912734e6d33b0`.
[actual.json](virtuoso-q5/actual.json) contains zero rows;
[reference.json](virtuoso-q5/reference.json) contains Jena's 14 rows.
[graph-count.json](virtuoso-q5/graph-count.json) records **10,102,032** triples,
matching the source graph count. This is a concrete bag mismatch in this setup,
not a datatype-only rejection. The proposed unbound-country compatible-join
cause remains an unreduced hypothesis, not a confirmed engine diagnosis.
The original archive retains all 12 q5 requests and bodies.

## Reproduction and validation

From the repository root, using the retained raw numerical-input directory:

```sh
python3 bench/analytical/rescore_value_diagnostic.py \
  --archive docs/analytical-benchmark/matched-vm-v1 \
  --reference-archive docs/analytical-benchmark/holdout-v1 \
  --numeric-inputs /path/to/verified/numeric-diagnostics-10m \
  --policy docs/analytical-benchmark/value-diagnostic-v1/POLICY.md \
  --out /path/to/new/output
python3 -m unittest discover -s bench/analytical -p test_value_diagnostic.py -v
```

The numerical input directory is the original freeze's uncompressed layout;
verify/decompress the retained numeric-freeze-v1 archive to reproduce it. The
runner verifies that freeze and its bound files/sources before scoring. It opens
no endpoint and makes no measurements. Source hashes are bound before scoring.
[summary.json](summary.json) gives the table; [requests.json](requests.json) has
all 720 scheduled entries, including 665 attempts, the timeout and 55 not run.
[input-identities.json](input-identities.json) records every archive/reference
file verified. Nine tests cover declared value spaces, invalid values, exact
boundaries without narrowing, wrong entities, duplicate counts/multiplicities,
binding state, frozen-envelope rejection and separate order failure.
[validation.json](validation.json) records the initial/final replay agreement;
[tests.log](tests.log) retains the final test output.
