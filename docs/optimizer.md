# Optimizer

**How the engine uses it today.** The executor orders each basic graph pattern greedily from exact index-prefix
counts. When statistics are available (persisted with disk stores, built in the background in memory), the pattern
has at most seven triples (`MAX_RUNTIME_DP_PATTERNS`) and the estimated greedy work is at least
`MIN_OPTIMIZER_WORK`, it uses `choose_variable_order` and the cost-based binary order described below. The `plan_bgp`
API keeps `enable_leapfrog` off by default; the executor chooses leapfrog itself for cyclic patterns. The sections
below record the design and its validation as it was built (task history: "P1").

The optimizer consumes dictionary-encoded `TriplePattern`s and statistics from
the same read snapshot. It emits scans, index nested loops, optional merge/hash joins, or a capability-gated
leapfrog plan. P1 costs are heuristic work units; they have not been calibrated
against elapsed time or shown to beat another RDF store.

## Integration

```rust,ignore
let mut stats = Stats::from_snapshot(snapshot)?;
stats.cache_pattern_counts(&patterns, snapshot)?; // exact index-prefix counts, no full query execution
let plan = plan_bgp(&patterns, &stats);
// Opt in to the leapfrog candidate:
let decision = plan_bgp_with_options(&patterns, &stats,
    PlannerOptions { enable_leapfrog: true, ..Default::default() });
```

`Stats::default()` describes an empty graph, not unknown/missing statistics.
Build statistics at bulk-load/open time and rebuild or version them on mutation.
Do not query a new snapshot using old exact-zero statistics. Query constants must
already have been resolved through the dictionary; an absent term can produce an
empty plan in the front end.

`JoinTree::Unit` is the identity relation (one empty binding), required by an empty
BGP. It differs from `Empty` (no bindings). The executor must implement both.
The default binary cost mode matches the executor: an index nested loop costs
outer work plus estimated outer rows times log2(graph size), plus estimated
output rows. Bound outer variables and constants lead the inner index prefix.
`BinaryJoinMode::Materialized` explicitly selects the alternative hash/merge model.

All shared variables are hash keys in that alternative model. Merge is emitted only when the inputs are
actually index/merge ordered on their single shared variable. Plans never assume
that a hash join preserves sorting.

## Statistics and estimates

- `Stats::from_spo` streams SPO-sorted triples, rejects out-of-order input and
  ignores adjacent duplicates. `from_snapshot` also validates batch-column lengths.
- Characteristic sets retain exact outgoing-predicate presence and subject counts;
  each matching set contributes subject count times mean matching multiplicities.
  This addresses missing-predicate correlations in subject stars. Degree and
  bound-object correlations **within** one characteristic set remain approximations.
  For example, the controlled 7-answer multiplicity fixture estimates 6 (q-error
  7/6), demonstrating that the synopsis is not an exact join counter.
- Predicate counts and distinct subjects per predicate are exact. Object-domain
  sizes use 128-register HyperLogLog sketches. Tagged IDs are deterministically
  mixed before sketching. This is an approximate domain model, not overlap sampling.
- Each predicate and each CS/predicate has 64-counter Misra-Gries object frequencies.
  Frequencies are exact before overflow; afterward midpoint estimates use the
  counter plus half the decrement count. An absent counter after overflow does
  not prove an absent object. Memory depends on predicate/CS counts and the largest
  subject group rather than retaining every triple or every subject ID.
- `cache_pattern_counts` gets exact constant-prefix cardinalities from the store;
  For rare predicate/object constants (at most 4096 matches) whose frequency
  sketch overflowed, bounded POS and subject lookups refine object counts per
  characteristic set. Fixed-subject stars use exact leaf-count products.
  Repeated-variable patterns retain synopsis/equality estimates instead of calling
  an unfiltered prefix count exact. Equality-mask counts handle unbound repeated
  variables. Chains/remaining shared variables use uniform-domain selectivity.
- Only an **exact zero scan** makes the planner eliminate a BGP. An estimated zero
  changes costs but is never a correctness filter.

## Search and physical choice

Small queries (default at most 10 patterns, hard cap 12) use subset dynamic
programming over left-deep plans, using compact cost/cardinality/predecessor arrays for index loops, with the winning
join tree built once. The optional materialized model retains the cheapest state
per available output ordering. Larger queries greedily extend connected patterns before cross
products. Hash builds use the estimated smaller input. The search space does not
include arbitrary bushy trees, sorts, parallelism or adaptive replanning.

The WCOJ candidate uses GYO reduction of the variable **hypergraph**: a subject
star is not called cyclic just because many relations share its center. Eligibility
requires at least three patterns, cyclic structure and no repeated variable within
a pattern. The cost proxy uses a feasible uniform fractional cover (weights equal
to reciprocal minimum variable degree) with estimated relation sizes, plus scan
work. The proxy has no measured accuracy guarantee. A cheaper proxy is selected
only if `enable_leapfrog` is true. Children use permutations whose unfixed columns
follow the selected global variable order. The default `plan_bgp` emits a binary
fallback until an executor explicitly opts in.

## Validation status and q-error contract

`cargo test --test optimizer` checks 21 integration cases, including exact prefix
counts, correlated stars, multiplicities, frequency overflow, repeated variables,
all constant masks, empty-BGP identity, physical sort/key invariants, both WCOJ
branches and 60 generated small graphs. A separate relational oracle compares
result multisets for DP and greedy plans. This oracle interprets plans; it is not
a claim that an optimized leapfrog executor has been implemented or benchmarked.
A library test checks sketch behavior on tagged sequential IDs.

`q_error(estimate, actual)` returns 1 for two zeros, infinity for exactly one zero
or invalid estimates, and otherwise max(estimate/actual, actual/estimate). Report
false-zero cases explicitly instead of hiding them with a +1 clamp. The benchmark
harness can call `stats.estimate_bgp(&encoded_patterns)` and compare against the
exact **BGP binding count**, not a DISTINCT/projection/LIMIT/filtered count with
different semantics. Repeat the comparison for plan subqueries to diagnose errors.

LUBM-10 validation is recorded in [optimizer-lubm10](optimizer-lubm10/README.md).
All 14 query result multisets agree between greedy and cost-ordered execution,
and counts match the existing cross-engine reference. The worst q-error remains
251 on Q9: cyclic correlations are not solved. WatDiv SF1 validation is in [optimizer-watdiv-sf1](optimizer-watdiv-sf1/README.md):
all 20 basic templates match Jena counts; four empty results have positive estimates
(infinite q-error), and positive-result median/max q-error are 2.714/24.
No runtime win or state-of-the-art accuracy is claimed.

## Research basis

[Neumann and Moerkotte, ICDE 2011](https://bda.snu.ac.kr/images/b/b9/Characteristic_Sets-_Accurate_Cardinality_Estimation_for_RDF_Queries_with_Multiple_Joins.pdf)
introduce characteristic-set synopses for RDF star correlations. This implementation
borrows the subject/predicate-set grouping and mean multiplicity idea, with bounded
object-frequency summaries and simplified chain estimates.

[Veldhuizen, Leapfrog Triejoin](https://arxiv.org/abs/1210.0481) establishes the
algorithm's worst-case optimality up to a logarithmic factor. That result applies
to an appropriate join implementation, not to this planner's cost estimates or
its choice between physical algorithms. Runtime integration must separately test
variable-order consistency, set/bag behavior and result equality.

Storage errors in statistics construction and exact-count refinement return `StatsError::Storage` with the original error as its source. Refinement commits its caches only after all requested reads succeed, so failed reads cannot leave partially refreshed exact counts.

## Preparation performance

Immutable characteristic-set and predicate synopses are shared through `Arc` when
`Stats` is cloned; each query retains its own exact-count caches. The public
`characteristic_sets` field is now `Arc<Vec<CharacteristicSet>>`; read-only indexing,
`len` and `iter` continue to work. The statistics builder uses copy-on-write mutation
before publishing the snapshot. A query's cache refinement cannot alter another
clone's estimates.

`--bench-planner` on the q-error example measures seven preparation samples per
saved query and prints median nanoseconds for clone, exact-prefix refinement,
plan search and total. Parsing, data loading, initial synopsis construction and
query execution are outside that interval. See `planner-performance/` for the
fixed-binary comparison and its limits.

## Cost-based trie variable order

`choose_variable_order(patterns, stats)` returns the global variable permutation
for trie joins. Up to 12 variables use subset DP; larger inputs use greedy prefix
extension. The work proxy sums projected-prefix binding estimates times the
number of participating relations and an index-seek factor. Projection sizes are
capped by estimated relation rows, with uniform NDV overlap for joins. This is a
heuristic cost model, not an AGM bound or a runtime guarantee; correlations and
rare-object conditioning remain approximate. It never filters query results.

`variable_order_work` scores a complete order and rejects omitted, duplicated or
unknown variables. Tests compare the small search with exhaustive permutations,
check the large-search permutation, and retain the relational oracle's trie-order
and result-multiset checks. The executor must use this returned order when it
constructs every relation's trie permutation. Supplying statistics is optional;
the engine's legacy order remains the fallback without them.

For engine-level trie-order verification, run the q-error example with
`--verify-wcoj`. It compares three complete result multisets and their independent
reference counts: index loops without statistics, legacy WCOJ without statistics,
and WCOJ with statistics and the cost-based variable order. Acyclic queries still
exercise their normal fallback. These checks measure correctness, not latency.
The standard CLI `query` entry point does not pass statistics, so it does not
implicitly enable the cost-based variable order merely by containing the API.

## Compiled star subsets

Bounded DP now compiles each eligible subject-star's subset estimates once per
query. Characteristic-set predicate lookups and ordered products are reused,
instead of rescanning the synopsis for every candidate subset. The largest
compiled star has 10 patterns (at most 1,024 subset slots). Unsupported stars and
larger searches retain the original estimator. This is query-local estimate
reuse, not cached answers or a cache requiring repeated queries. Exact-prefix
refresh invalidates any compiled estimates.

The recurrence appends the highest selected pattern to its lower-index prefix,
preserving the original floating-point multiply/divide/clamp order. A unit test
checks bitwise equality across 756 generated star subsets. Estimator limitations,
including the Q9 error, remain unchanged.

For opt-in diagnosis, `TRIPLELIGHT_PROFILE_PLAN=1` prints clone, exact-count/refinement,
search, and complete `planned_order` nanoseconds. `optimizer_hotpath` reports whole
query time without serialization. Subtracting preparation gives parsing,
execution and materialization together, not a pure operator/kernel time. Normal
requests do not acquire these diagnostic clocks or write profile output.

The optional statistics path first estimates greedy work from per-pattern sizes
and uniform domains, without CS enumeration or snapshot reads. Below 100,000
approximate index seeks plus matches it keeps the existing greedy (or legacy
trie) order; above it, cost planning remains eligible. This is a policy threshold,
not a latency guarantee. Misestimation can skip a useful plan or admit a costly
one, but cannot change answers. Query-local compilation reuses per-pattern NDVs;
rare-object refinement uses a shared CS-signature hash index instead of scanning
all CS records for every subject.

## Runtime search budget

The server admits cost planning only up to seven triple patterns (at most 128
binary-DP subset states). Wider BGPs retain the executor's original exact-index-
count greedy order; wider cyclic BGPs retain its legacy trie variable order.
This bound is checked before cloning query statistics, refining exact counts or
compiling star subsets. C3's six-pattern optimization remains eligible.
Library callers retain the configurable DP/connected-greedy search APIs.
The policy trades opportunities on wide queries for bounded server preparation;
it is not a claim that wide queries cannot benefit from cost optimization.
