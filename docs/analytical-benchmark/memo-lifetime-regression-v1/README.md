# Memo lifetime regression: rejected q4 candidate

Candidate `1e862e39e08acc9074b9d8460a3a34c21ae0e41f` fails an independent tiny-graph correctness gate before its scheduled BI requalification. A query with twelve alternating UNION branches returns **11 rows**, while both main `738f0cb` and Apache Jena 6.2.0 return the expected **6**. The complete result bag fails, so no merge gate or speed claim is cleared. The 60-instance BI run was not started after this failure.

The three-triple graph has two predicates binding the same IRI to `?a` and `?b`, and one present EXISTS predicate. Each UNION branch includes `FILTER(?a=?b && EXISTS { ... })`; the EXISTS predicate alternates between the present predicate and an absent one. Exactly the six present-predicate branches should return a row. The full query, data and all three raw outputs are retained under `fixture/`.

Source review points to raw AST-address memo keys. The equality rewrite constructs temporary cloned filter expressions, which are dropped after evaluation. A later clone can reuse a graph-pattern address while query-wide memo tables still retain that address's old result or variable-index list. The reproduced answer error is established; this allocation/lifetime explanation is a source-level diagnosis rather than an instrumented allocator trace. The aggregate argument memo also uses expression addresses and needs the same lifetime review for cloned patterns.

The candidate was built from isolated pinned source with one Cargo job. Its binary SHA-256 is `f81e8f9dbfe2995bdc36701fc54c2f8869ceadb3bad625ec3b32a4e09d6ff7a6`; all 33 selected source files match after the check. Source/build metadata and the merge-gate receipt are retained; native binaries are excluded. All diagnostic processes exited and the owned benchmark lock was released for the fix. No frozen validator or old benchmark outcome was changed.

The exact row count may depend on allocator reuse; any extra/missing row is a regression. A corrected candidate must match the independent oracle on this fixture and pass the full frozen BI cohort before its performance result can replace the last qualified main table. Kaiming owns the engine fix.
