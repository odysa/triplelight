# Plan quality at 10x scale (task 17)

Status: 93 all-target tests passed on source 1652fe5, and release tools compiled. Tiny disk smoke queries verified full multiset equality with profiling enabled/disabled (20 paired output hashes), actual leapfrog and greedy trace events, and the COUNT collection pipeline against hand-counted data. Raw checks are in validation/. The collector smoke server was rdfdb, so this is protocol validation, not independent large-data correctness evidence.

The first LUBM-1000 synopsis/plan run is complete: see LUBM1000.md. All 65 BSBM-100M BGP probes are exported; their exact counts and full-query runtime comparisons are pending. The task-15 statistics-build measurement is separately pinned in ../exact-tag-coverage/MEASUREMENTS.md.

## Questions and interpretation

For each LUBM-1000 and BSBM-100M query report:
- Each BGP's standalone exact cardinality and raw/refined estimate, q-error and estimated greedy work.
- Candidate binary join tree, search method and the 100,000-work / 7-pattern runtime gate.
- Full-query statistics-on/off latency split using the existing opt-in phase trace and exact complete outputs.
- Cross-reference the owner's round-9 per-query times under bench/results/r9-*, recording the actual commit, dataset receipt and query hashes.

A candidate plan is not proof that execution selected it. Cyclic, initially unbound BGPs can use leapfrog; larger queries and low-work queries retain runtime greedy ordering. Bound OPTIONAL inputs can change the runtime order. The candidate trace labels binary gate eligibility, not observed dispatch. The draft adds opt-in bgp_actual events in exec/bgp.rs: actual binary pattern order and pre-join/extended/post-filter row counts, plus the final leapfrog variable order after fallback selection. They compiled and the leapfrog/greedy events were exercised on the preserved tiny fixture; planned dispatch at scale remains pending. Empty/absent-constant and zero-variable membership early exits do not emit these events.

A high q-error does not by itself establish a bad order. Separate preparation/search time from execution, then check whether poor estimates change an order and increase intermediate work. An open gate and slower full query can reflect planning overhead, an order regression, or both. Compare those components at both scales before changing the threshold.

## BGP reference contract

examples/extract_bgp_probes.rs walks the parsed source algebra and emits one isolated SELECT and one COUNT probe per BGP, with source SHA-256 and algebra path. It does not pass FILTER, DISTINCT, ORDER BY, LIMIT, OPTIONAL or UNION result counts off as BGP counts. OPTIONAL-right probes are standalone graph cardinalities, not counts conditioned on incoming left bindings. They are useful synopsis diagnostics but cannot alone measure bound-input join error. GRAPH, SERVICE, paths and EXISTS sources fail explicitly rather than silently change scope. The new output directory must not already exist.

collect_bgp_counts.py sends only the generated COUNT queries to an explicitly supplied reference endpoint (prefer Jena loaded with the same dataset). It records every exact response and its hash, the query hashes, cohort manifest hash and operator-supplied dataset receipt. The script does not verify the endpoint dataset independently; establish that through the load receipt before running. It refuses stale output directories and differing manifest/query cohorts. Do not use a full-query LIMIT count here.

optimizer_qerror now accepts an immutable disk snapshot directory, avoiding a second 100M-row in-memory store. --plan-trace prints the BGP pattern list, candidate binary tree and gate values alongside the existing per-BGP q-error TSV. It still rejects modified queries; feed the generated isolated SELECT probes and their own exact counts.

optimizer_hotpath accepts a disk snapshot as well. It runs complete original SELECT queries with the original modifiers, alternates statistics/no-statistics order for five samples, checks the full result multiset across samples/modes and records sequence and multiset hashes outside the measured evaluation. This measures direct API work, with resolved terms, and opt-in trace overhead; it is not the HTTP timing protocol. A LIMIT result mismatch can involve ordering ties and must be investigated rather than automatically blamed on the planner.

## Execution sequence when the machine is released

1. Fresh-read the round-9 directive and atomic benchmark lock; acquire your own lock before builds or scans. Never remove the owner's lock.
2. Compile/test the changed examples with a single build job. Smoke-test extraction, exact reference collection and disk execution on a tiny fixed graph, including nested OPTIONAL and rejected modifiers.
3. Build task-15 baseline/candidate statistics probes from the same source except coverage metadata, then compare build overhead and estimate/tree signatures on the existing smaller snapshots. Do not conflate this with task-17 planner changes.
4. Pin round-9 data, disk format, reference source, code and query cohorts. Export BGP probes for each scale, collect independent exact counts, then run q-error and phase profiling serially. Use a small query subset first to avoid an unbounded execution.
5. Preserve all traces, hashes and per-query tables before proposing a planner change. Runtime plan tracing is a small exec/bgp.rs patch; rebase it against the numeric-filter branch before integration.
