# Measure decision sensitivity before repairing a cardinality estimate

A large q-error is a synopsis diagnostic, not evidence that a different plan would be faster. First verify that the query actually uses the suspected planner path and that the gate is open. Then change only the decision being questioned and compare complete results outside the timing interval.

For a three-variable cyclic BGP, all six leapfrog variable orders give a practical payoff bound. Compare the current selected order against the best forced order through the same production kernel, with one fixed snapshot/Stats instance, warm-up and interleaved repetitions. Preserve every raw sample and exact result multiset. A candidate binary plan is not an observed leapfrog order.

In the Nov 17 LUBM-1000 check, Q9/Q2 had large complete-BGP estimate errors, but the chosen Y-Z-X order was already the fastest of six for both. The declared 1.5x recoverable-payoff threshold was not met, so those estimators were left alone. See ../docs/scale-plan-quality/FORCED_ORDERS_RESULT.md for the scope and measured evidence. This conclusion applies to those tested decisions, not all queries or later dictionary rebuilds.

Count semantics matter: Snapshot::count counts matching triples, not generally the distinct next-key values. Exact variable-domain sizing needs the appropriate distinct/rank/seek contract; do not rename row counts as exact NDV.

## Use the algorithm the query actually dispatches to

The checked WatDiv/BSBM cohorts were acyclic, so forcing leapfrog would have changed the algorithm. Compare their actual index-nested-loop pattern orders through the complete evaluator instead, preserving filter pushdown and final modifiers.

A bounded follow-up sampled 24 connected pattern orders each for BSBM-100M q010/q013 and WatDiv C3. The selected order was best within every tested family; no 1.5x payoff appeared. See ../docs/scale-plan-quality/PATTERN_ORDERS_RESULT.md. This is not a global-optimality proof: only 24/396 or 24/720 eligible orders were sampled. Preserve that qualifier when citing the result.

Keep normal unforced controls separate from fixed-order comparisons. A difference between a diagnostic entry and a normal API call includes instrumentation and cache/position effects; it is not automatically isolated planning time. Preserve exact projected multisets, including duplicates and LIMIT-selected rows, and investigate any ORDER BY boundary ties rather than silently discarding an arm.
