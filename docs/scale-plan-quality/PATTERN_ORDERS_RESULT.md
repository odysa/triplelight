# No large ordinary-order payoff in the three requested queries

The current order was fastest among the **24 tested orders for each of BSBM-100M q010, q013 and WatDiv C3**. Selected/best fixed-order median ratios are **1.00× for all three**, so none reaches the requested 1.5× threshold. Together with the exhaustive six-order checks on the three-variable LUBM Q9/Q2 BGPs, no measured order change is warranted in this task.

This is a bounded candidate-family result, not proof of global optimality or proof that every other latency gap is caused by storage/decode.

| Original query | Connected family / tested | Selected fixed median, ms | Best tested median, ms | Selected / best | Ordinary control median, ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| q010 | 396 / 24 | 93.131 | 93.131 | 1.000× | 89.098 |
| q013 | 396 / 24 | 39.300 | 39.300 | 1.000× | 32.463 |
| C3 | 720 / 24 | 126.844 | 126.844 | 1.000× | 114.667 |

The BSBM selected order is [3,5,1,2,0,4,6]: source numeric properties 1 and 2, source feature, product feature, label, product numeric properties 1 and 2. C3 selects [2,3,4,5,0,1]: Location, age, gender, givenName, likes, friendOf. Pattern indices refer to the unchanged parser/source order. Each selected order was itself included in the tested family.

## Completed comparisons and semantics

All **525 samples** completed: 504 fixed-order samples and 21 unforced controls. Excluding one warm-up per arm leaves 432 fixed-order measurements and 18 controls. Every complete projected ID multiset matched the ordinary query reference, including duplicates: 5 rows for each BSBM query and 429,276 for C3, also the earlier Jena reference row counts.

The benchmark retains the original full query: FILTER pushdown, projection, DISTINCT, ORDER BY and LIMIT run through the same evaluator. It does not time a stripped BGP and compare that to a limited final result. Canonicalization, result hashing and comparison are outside the timer. No output-mismatch arm was dropped, and there were no timeout-censored arms.

Forcing selected and alternative orders through the same diagnostic entry isolates the payoff of choosing another tested order from the cost of selecting an order. The ordinary controls run separately, alternating before/after the fixed batch. Their different timings include a different diagnostic/control path and cache/position effects; **subtracting the table columns is not a pure planning-time measurement**. They provide no evidence here of a large additional planning-path penalty.

The source-salted, first-pattern-stratified sample retains the actual selected order plus up to 23 connected alternatives. It covers 24/396 connected orders for each BSBM query and 24/720 for C3. It does not guarantee coverage of every local neighbor or every possible predicate arrangement. Definitions and the full recorded candidate lists are in PATTERN_ORDERS.md and each raw/metadata.txt.

## Source and resources

Actual measured source is **0087875**, preserved as tag **task17-pattern-orders-measured**. Both processes used one in-memory snapshot and one Stats instance for the relevant dataset, with 10 Rayon workers; profiling and disk-cache counter flags were unset. Repetition is one warm-up plus six rotated/reversed measured passes.

BSBM loaded 98,861,444 triples in 210.68 s; Stats took 23.21 s. The complete process took 587.08 s. Maximum RSS was 28,659,662,848 bytes (26.69 GiB); macOS peak memory footprint was 55,145,526,032 bytes (51.36 GiB).

WatDiv loaded 10,914,314 triples in 14.17 s; Stats took 3.18 s. The complete process took 82.33 s. Maximum RSS was 5,810,929,664 bytes (5.41 GiB); peak memory footprint was 4,579,398,432 bytes (4.27 GiB). These OS metrics differ and include graph construction; neither is just query memory or a configured hard cap.

Each process exited 0 within its 900-second limit. Input and binary hashes matched before/after. Query, helper and execution-source hashes were rechecked after both jobs. Full source/input/binary receipts, raw resource output, all repeats and multiset hashes are retained under pattern-bsbm100/ and pattern-watdiv100/, with reproducible summary code in summarize_pattern_orders.py.

This was one shared-host session under the RDF benchmark lock. It controls the team's concurrent builds and query timing, not every background process or all host state. The reading is that **these tested choices offer no ≥1.5× order improvement**. Preserve the result as a useful negative finding and leave production order selection unchanged; do not generalize it to untested queries, all connected permutations or the cause of every remaining gap.
