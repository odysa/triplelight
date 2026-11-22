# Pilot 3: degree-based result-size upper bounds on LUBM-100 (offline validity check)

Task #99 (debate #95, idea 1). A check of the idea, not a contribution and not a performance result.

**Data.** `lubm100.nt`, UBA 1.7 seed 0, 2,377,133,547 bytes, SHA-256 `49eb4a91...` (the file pinned by the #72 registration), read in place.
13,879,970 statements, 18 predicates. `stats.py` counts statements, so duplicate statements (about 3% in LUBM) inflate degrees: the bounds stay valid and may be slightly loose.

**Rule (registered in the #99 thread before any statistic was computed).** Bound (a) is the product of per-step factors in a greedy join order:
start from the atom of smallest exact cardinality; add next the connected atom with the smallest factor; a fully bound atom has factor 1; otherwise the factor is
the max degree D_s(p) or D_o(p), or the exact count when an end is a constant. Bound (b) would replace the first join with the l2 (Cauchy-Schwarz) bound.
In this rule every query's first join involves a constant or an `rdf:type` atom, so (b) never applied and the reported bound is (a). Implemented in `bounds.py`.

**Truth.** `truth.py`: exact set-semantics counts by in-memory index joins in Python (an offline oracle, independent of the engine).
The registered engine `COUNT(*)` run (`engine_counts.txt`, release binary of main at `fbffe93`, queries wrapped as `SELECT (COUNT(*) AS ?n) WHERE {...}` over `lubm100.nt`, run under the lock) reproduces all 14 counts in `truth_py.json` exactly. The file has 13,409,395 distinct triples out of 13,879,970 statements.

| Query | true count | bound | bound / true |
|---|---:|---:|---:|
| Q1 | 4 | 4 | 1.0 |
| Q2 | 264 | 804,807 | 3048.5 |
| Q3 | 6 | 6 | 1.0 |
| Q4 | 10 | 41 | 4.1 |
| Q5 | 532 | 678 | 1.3 |
| Q6 | 795,970 | 795,970 | 1.0 |
| Q7 | 59 | 208 | 3.5 |
| Q8 | 5,916 | 11,040 | 1.9 |
| Q9 | 4,462 | 1,851,552 | 415.0 |
| Q10 | 4 | 4 | 1.0 |
| Q11 | 10 | 10 | 1.0 |
| Q12 | 125 | 630 | 5.0 |
| Q13 | 265 | 325 | 1.2 |
| Q14 | 795,970 | 795,970 | 1.0 |

**Result against the registered rules.** Valid on 14/14 (hard requirement met). Within 100x on 12/14 (signal needs at least 10). Median ratio 1.25 (abandon needs above 10^4), geometric mean 4.0.

**Limits.** Six queries (Q1, Q3, Q6, Q10, Q11, Q14) have an exact bound. Six more (Q4, Q5, Q7, Q8, Q12, Q13) are within 5x; they are all anchored by a constant.
The two cyclic or multi-class joins, Q2 and Q9, are the loose ones (3048x and 415x), and they are the queries a memory guard matters for. Only the 14 no-inference LUBM queries were tested; WatDiv-100 was not part of this pilot.
