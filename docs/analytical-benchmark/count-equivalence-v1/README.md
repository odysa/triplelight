# COUNT integer-value equivalence, version 1

This optional policy is frozen before inspecting QLever’s confirmatory BI
outputs. It reports alongside the unchanged strict datatype result. Its scope
is only the computed COUNT columns of BI q1, q2 and q5; xsd:int and xsd:integer
with the same valid integer value are equivalent. See freeze.json for exact
scope, lexical/value-space requirements, authorization and source hashes.
The binary32 numeric envelope remains separately frozen and unchanged.

The HTTP runner requires both --count-policy-freeze and the preregistered
--count-policy-sha256. It verifies the policy and source identities before any
request. Raw results are unchanged. Strict failures suppress the strict total;
a separately labelled equivalent total requires every scheduled equivalent
result to pass. Neither total exists after any missing or failed result under
its own rule.

Five normalization tests and nine HTTP/adapter checks pass, including an
integration check where a strict datatype failure remains a failure while the
separate equivalent result passes. This is test evidence, not large-graph
QLever qualification.

Freeze SHA-256: ffa49a6ce10c19c02d6185a3431383157c590e1e1beb9c9d37b7b83e03bf5790
