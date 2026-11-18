# Retrospective numeric value diagnostic v1

Requested after the raw competitor outputs were seen. This is a descriptive
re-score, not a preregistered qualification, a change to the frozen rules, or a
new performance ranking. Strict and COUNT-equivalence scores remain unchanged.
Policy source and this text are hashed before executing the re-score.

Only designated computed numeric and COUNT output columns lose datatype identity.
All other RDF terms, bound/unbound state, projected variables, row multiplicities
and q7 cutoff rules remain. COUNT values must be exactly integral and nonnegative.
Accept valid XSD integer-family, decimal, float and double literals. Decode decimal
as its exact rational value, float as nearest/ties-even binary32, and double as
nearest/ties-even binary64. Reject invalid lexical forms and non-finite values;
never narrow decimal/double output to binary32 to make a bound pass.

For q4/q5/q6 both Jena and the competitor must lie within the original exact
rational anchor envelopes from numeric freeze a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244.
For q3/q8 use the closed interval whose endpoints are four ordered binary32 steps
either side of Jena's binary32 value, retaining the original four-step tolerance.
This is approximate numeric compatibility within existing bounds, not exact
mathematical equality. Numeric bag checking and outer ordering are reported
separately; a combined pass requires both. Missing/failed transport is never a
value error or pass. No qualified timing totals are created by this re-score.

Virtuoso's q5 raw outputs, exact query texts and graph-count response are retained
in the original matched-vm-v1 archive. The new report will link the evidence and
provide a compact hash-bound example. No diagnosis of the underlying engine is
inferred from datatype rejection or from the unreduced q5 mismatch.
