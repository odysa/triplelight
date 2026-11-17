# Proposed numeric validation for the next BI cohort

Status: **mathematical approach approved; implementation not yet qualified or frozen**.
Kaiming reviewed the proposed assumptions in message d1044631.
`bench/analytical/float_envelope.py` and its tests are a prototype, not connected
to `qualify.py`; tests and graph diagnostics still need an exclusive window.
No new candidate outputs may be inspected under the revised rule before its
source and input diagnostics are frozen.

The rule is not applied. The first 15-instance cohort retains its
original four-binary32-ULP gate and all failures. This document does not turn any
existing failed result into a pass.

## Why the next cohort needs a different numerical check

The first real BSBM-10M q4 instance returns the same ordered ten features on
Jena, DuckDB and rdfdb. Maximum binary32 distances from Jena are 7 ULP for
DuckDB and 27 for both rdfdb aggregation paths. Relative differences peak at
7.68e-7 and 2.93e-6 respectively. Both rdfdb paths return identical complete
term bags on all fifteen instances. These observations alone identify neither
a wrong aggregate nor the precise cause of the differences.

[SPARQL's SUM definition](https://www.w3.org/TR/sparql11-query/#defn_aggSum)
reduces numeric inputs with numeric addition; AVG uses SUM and COUNT. Inputs
are multisets. Different evaluation orders and accumulation precision are a
plausible explanation for differences on large float groups. A fixed four-ULP
comparison to one engine does not account for group size or cancellation.
This is a hypothesis pending independent arithmetic checks, not permission to
ignore arbitrary numeric discrepancies.

## Proposed rule, to freeze before the next candidate outputs

The next confirmatory measured mixes are the three previously unexamined mixes
1–3 in the exported workload. Mix 0 has already been inspected and remains
exploratory. Freeze the validator source, diagnostics, workload and runtime
identities before inspecting outputs from mixes 1–3. Any later revision creates
a new named cohort; retain the prior outputs and decisions.

Keep RDF datatypes, source identities, integer counts, duplicate multiplicities,
returned entity membership and per-engine ORDER BY checks exact. Keep the q7
allowed-cutoff-set oracle. This proposal changes only SUM/AVG-derived numeric
output checking; scalar count ratios retain the existing declared gate.

For each affected group, collect untimed input values and cardinalities from
the pinned graph. Independently cross-check group cardinalities with Jena.
Construct an exact rational sum `S` and sum of input magnitudes `A` after the
query's binary32 casts. Do not use another floating AVG as the anchor. Preserve
input identities and the diagnostic hashes. An invalid/unbound aggregate input
keeps its existing SPARQL error/unbound treatment; it must not be omitted from
the diagnostic or repaired into a number.

For finite binary32 arithmetic let `u = 2^-24`, `delta = 2^-150`, and
`gamma_n = n*u/(1-n*u)`. A conservative absolute summation envelope is

```
E_sum = gamma_n*A + n*delta/(1-n*u)
```

This accounts for order-sensitive rounded additions and an absolute underflow
allowance. It requires `n*u < 1` and no overflow. The count-to-float conversion
must be exact (`n <= 2^24`) for the simple AVG rule below. Otherwise use a
separately derived denominator interval, or mark the case indeterminate.
Do not apply a relative-to-result bound to a zero or cancelling sum.

For the exact mean `m = S/n`, conservatively propagate division rounding:

```
E_avg = E_sum/n + u*(abs(S)+E_sum)/n + delta
I_avg = [m-E_avg, m+E_avg]
```

For q4, independently derive intervals for the with-feature and without-feature
means. Only if the entire denominator interval is positive may interval division
produce a ratio interval; then add one binary32 division rounding envelope.
If an interval crosses zero, arithmetic overflows, input cast semantics are
unresolved, or diagnostics do not agree, the check is **indeterminate/failing**,
not a pass with a larger tolerance.

Test every engine, including Jena, against the same exact-anchor envelope.
Report each output's absolute, relative and ULP differences from the anchor and
from Jena. Preserve observed differences even when they lie inside the envelope.
Numerical membership or HAVING/ORDER sensitivity that changes entity rows is a
separate discrepancy; this proposal does not silently admit alternative rows.

## Required verification before adopting the rule

The arithmetic helper needs tests for exact sums, long same-sign sums,
cancellation, zero, subnormals, non-finite values, invalid inputs, count-cast
limits and denominator intervals containing zero. A deliberately corrupted
large output must fail. Check q4's two input multisets/cardinalities and both
AVG components before attributing the current difference to accumulation order.
The engine owner should review the operation-count and rounding assumptions.
Until that work is complete, publish no full BI performance score from the
failed cohort. This is a benchmark validation dependency, not a claim that
q4 is unsupported by the engines.
