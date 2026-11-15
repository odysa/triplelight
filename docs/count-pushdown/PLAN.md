# Count pushdown — unvalidated draft

Task #13. The implementation and fixtures are edited, **not compiled or measured** while round 5 owns the shared-machine lock. No performance or correctness claim yet.

## Supported fast paths

- A single pure triple pattern with no repeated variable: exact `COUNT(*)`, `COUNT(?x)`, and `COUNT(DISTINCT ?x)` from index prefix counts and distinct-key seeks.
- `GROUP BY` variables of that pattern: permute constants and grouping keys to the front, enumerate group-key runs, and count their ranges. Unbound group variables and empty-input global/grouped aggregates retain their separate behavior.
- Ungrouped pure BGP `COUNT(*)`/`COUNT(DISTINCT *)` and bound-variable `COUNT(?x)`: count complete leapfrog bindings, using the last variable's intersection size without materializing output rows. Independent BGP components multiply exact counts instead of walking their Cartesian product.

All aggregate requests must be recognized, and there must be one completely unbound input row. Mixed aggregate functions, FILTER/OPTIONAL/subqueries inside the group, repeated-variable patterns, grouped joins, and distinct projected-variable join counts use the existing evaluator. Distinct-star with pattern blank nodes also falls back: existential blank nodes need projection-aware treatment. Integer overflow reports a storage failure; storage errors are never turned into zero.

Term IDs remain opaque RDF-term identities. Different lexical forms and datatypes must remain distinct even if their numeric values compare equal. [SPARQL 1.1 aggregate definitions](https://www.w3.org/TR/sparql11-query/#defn_aggCount) are the semantic reference.

## Pending verification

1. After fresh-reading the active directive and `/tmp/rdf-bench.lock`, acquire the lock atomically. Test the new aggregate suite, then all targets. Resolve compilation and semantic failures before claiming readiness.
2. Build baseline and candidate from the same main base, with the exact same `examples/count_probe.rs`. The candidate must differ only by count pushdown. Use matching single-core builds and runtime worker counts; preserve source/binary/query/manifest hashes.
3. `make_cases.py` creates 72 queries: 34 COUNT-star rewrites, 34 DISTINCT-first-variable cases, and four direct prefix-group cases. It refuses DISTINCT, filters, and solution modifiers in the original source cohort. COUNT-star answers must match the saved external round-4 row counts. All baseline/candidate canonical complete outputs must match, not just result row counts.
4. The probe does one unmeasured warmup and five measured API queries, with normalization/hashing outside the query timer. Preserve full small answers and hashes of larger grouped answers. Compare totals and each query; use repeated alternating variant sessions if any difference is small or a regression appears. Original 34-query totals should also be checked.
5. Use Jena for new DISTINCT/group correctness fixtures when the benchmark machine is available. Existing COUNT-star reference counts are derived from the all-six-system agreement; do not imply Jena's aggregate query was rerun if it was not.
6. Release the lock before offline analysis. No default merge until correctness and same-session timing evidence support it.

`analyze.py FAMILY BASELINE.tsv CANDIDATE.tsv` requires all generated cases, five stable samples each, exact cross-variant answer hashes, and 34 aggregate-count references across both families. The new example and all generated fixtures are currently uncompiled drafts.
