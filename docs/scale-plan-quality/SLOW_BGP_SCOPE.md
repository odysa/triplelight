# The next slow cohorts do not dispatch to leapfrog

The parser-exported WatDiv cohort contains 20 BGPs and BSBM-100M contains 65 BGPs across 20 source queries. **All 85 BGPs are acyclic.** This is structural, independent of synopsis estimates or data size.

inventory_bgp_shapes.py verifies that every token in each exported BGP is accounted for and that token counts equal three times the parser-reported triple count. Its GYO cycle reduction agrees with an independent union-find forest check on these cohorts' unary/binary variable hyperedges. The complete manifest, source hashes, algebra paths and shape results are in bgp-shapes.json. The inventory is static, not a runtime benchmark.

The production dispatch condition in exec/mod.rs requires a cyclic BGP (in addition to three or more patterns, no repeated-variable triple and one fully unbound input row). Thus these BGPs cannot take that leapfrog path in the current executor.

| Slow source | Patterns | Variables | Cycle | Current algorithm family |
| --- | ---: | ---: | --- | --- |
| WatDiv C3 | 6 | 7 | No | Ordinary index nested-loop joins |
| BSBM-100M q010 | 7 | 7 | No | Ordinary index nested-loop joins, with pushed filters |

There are not three variables to permute in either example. Forcing the leapfrog kernel would change the algorithm, so its result would not bound an order improvement in the current ordinary-join path. BSBM's FILTER/DISTINCT/ORDER/LIMIT semantics also must be preserved.

A relevant follow-up is a bounded ordinary-pattern-order sensitivity comparison on the complete original queries, using their actual filter path. Its six candidates would need explicit definitions and could not be called an exhaustive six-permutation optimum over these larger BGPs. No such new benchmark has run yet, and no production planner change follows from this inventory.
