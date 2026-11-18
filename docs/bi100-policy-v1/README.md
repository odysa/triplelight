# Prospective whole-result policies for BSBM 100M BI

Acceptance preregistration SHA-256: `7ce4cc804af9c74408e38355bd88319bcf22456024f2bcc5a7bed958957a9927`.

COUNT-only policy SHA-256: `5049fb75210fc6fc78ebf1fb2553b88baef197ae73495b70f110a203a70a4663`.

These policies were fixed before collecting any 100M BI reference or candidate outputs. They retain separate strict, COUNT-compatible and value-compatible results. The arithmetic and whole-result rules were developed on the prior 10M cohort and remain unchanged; this new scale is not a blind algorithm-design holdout. The separately verified 100M exact-anchor freeze is bound explicitly.

Value checking interprets the declared numeric datatype without narrowing decimal/double to float. q4/q5/q6 retain the original exact-input-derived rational envelopes; q3/q8 retain four-binary32-step endpoints around the independent Jena reference. Integral nonnegative COUNT and all other term identity, multiplicity, binding, order and cutoff checks remain required. All 60 scheduled requests must pass before the total of 45 measured requests can be reported. No partial or failed-query total qualifies.

The independent reference outputs do not exist yet. Their hashes will be bound into the runnable policy after collection; this does not permit changing the preregistered rules, validator source identities, workload or exact-input freeze. All failures and timeouts remain retained. This document reports no engine correctness or performance result.
