# Bounded ordinary-join order payoff check

Status: 100 all-target tests passed on the draft, including modifier/error boundary tests and connected-candidate selection checks. All three large-data comparisons completed with exact projected-multiset parity; see PATTERN_ORDERS_RESULT.md. Kaiming's scope is only **BSBM-100M q010 and q013, and WatDiv C3**, with about 24 connected pattern orders per query. This replaces the inapplicable three-variable leapfrog enumeration for these acyclic queries.

## Semantic path

exec/diagnostic.rs offers an explicit library-only order override and an actual-pattern-order recorder. The ordinary HTTP/query entry does not request either. The diagnostic entry accepts an acyclic single-BGP SELECT and validates a full permutation; nested graph/subquery/EXISTS execution and cyclic BGPs are rejected. It reuses the complete evaluator, so original FILTER pushdown, projection, DISTINCT, ORDER BY and LIMIT are retained. It preserves storage-error classification. The draft adds three focused tests for those boundaries.

Bgp::eval_greedy and eval_order record pattern indices only when an optional diagnostic recorder is present. This recorder is a mutex shared by one query, updated once per join step, never once per row. Normal requests allocate no recorder; the new ordinary-path branch overhead remains unmeasured and is not being declared zero.

## Candidate family and comparison

The helper first runs the normal query and captures its actual full pattern order. Empty/early-stopped executions fail closed rather than invent a complete baseline order. Its complete result ID multiset (including duplicate rows, projected variable order and LIMIT-selected rows) is the reference for every sample. Computed projected scratch terms are outside this narrow comparison and are rejected.

Enumerate syntactically connected orders: if a remaining pattern shares a currently bound BGP variable, extend through such a pattern; if none does, start a new connected component. FILTER expressions still run normally but do not add edges to this BGP-connectivity rule. The normal selected order is included even if the runtime's soft cross-product penalty made it fall outside this strict connected family.

Keep at most 24 candidates including the selected order. Remaining candidates are sampled round-robin across first-pattern groups, with a deterministic SHA-256 rank salted by the exact query source. Record the full eligible count and sampled list before timing. This is a bounded, reproducible sensitivity screen, **not an exhaustive optimum bound** for six/seven patterns.

Each fixed order warms once then has six rotated/reversed measurements. Full-query evaluation is timed; result canonicalization, hashing and comparison are outside timing. The selected fixed order is the denominator when comparing alternative fixed orders, so both use the same diagnostic validation/capture path and omit order selection. An ordinary unforced control runs once per pass, alternating before/after the fixed arms; it is reported separately so remaining planning-path overhead is not silently classified as storage/decode time. Its timing is not a pure isolated planning measurement.

The source full queries are unchanged. Any output mismatch stops the experiment; ORDER BY ties at a LIMIT boundary must be investigated before calling it a wrong plan. No mismatch arm is silently discarded to improve a best-time claim.

## Scope of the reading

Compare selected fixed-order median / best tested fixed-order median against the requested 1.5x payoff threshold, with all raw repeats and spreads. A timeout or incomplete candidate set is censored; do not claim all tested arms finished. If no tested query exposes a 1.5x improvement, the supported conclusion is no large order payoff **within this candidate family**. Stronger claims that all plans are globally optimal, or that every remaining gap is storage/decode, need separate evidence. Ordinary controls can reveal other planning-path costs.

Use one in-memory graph and one synopsis for q010/q013, then a separate WatDiv run for C3. Reuse run_forced_orders.py as an own-PID/time-bounded supervisor, preserving full input/binary/source receipts, completed samples and macOS RSS/footprint output. All compilation and execution wait until round 10 releases the shared benchmark window.
