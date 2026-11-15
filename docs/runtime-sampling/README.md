# Runtime random-walk estimator: do not enable by default

Task #12, 2026-11-15. This is a **negative deployment decision**, not a new serving speedup. A working, opt-in estimator and calibration harness are provided. Sampling repairs one severe estimate but misses other nonempty joins and cannot justify replacing the existing engine choice. No executor routing, planner gate, or default query option was changed.

## Measurements and decision

One macOS host session under `/tmp/rdf-bench.lock`, on the existing round-4 disk snapshots. Each query uses five fixed sampling seeds, 256 walks each, and five alternating full executions with WCOJ disabled/enabled. OS caches were not cleared. The research sample runs precede execution, warming caches. These are raw-ID API timings, **not HTTP/JSON round-4 timings**, and cannot be compared directly to round-4 totals.

| Query / workload | Static q-error | Median sampled q-error | Median sampling time | Limitation |
|---|---:|---:|---:|---|
| LUBM-100 Q9, 4,462 rows | 2,954.35 | 1.74 | 12.900 ms | One of five seeds returns zero; other seeds have only 3–5 successful walks |
| LUBM-100 Q2, 264 rows | 2.28 | infinity | 4.528 ms | All five seeds return zero |
| WatDiv100 C1, 8 rows | 39.64 | infinity | 1.243 ms | All five seeds return zero |
| WatDiv100 S4, 1 row | 439.18 | 4.95 | 0.258 ms | Two of five seeds return zero |
| WatDiv100 C3, 429,276 rows | 1.00 | 1.16 | 3.352 ms | Sampling adds time and worsens an already accurate estimate |

The earlier Q9 error of 251 was on **LUBM-10**. This experiment uses **LUBM-100**, so its same-data static baseline is 2,954.35, not 251. Q9 sample estimates are `[0, 7768.375, 10581.0625, 5491.4375, 5290.53125]`; a favorable median must not hide the failed seed or the large standard errors.

A separate **experimental** cap of 500 microseconds/4,096 index probes completed 55/70 LUBM samples and 86/100 WatDiv samples after warmup. This is an extra research budget, **not an existing production time gate**: production uses the 100,000-work threshold and seven-pattern search limit. Q9 fails this time budget in every seed. A successful deadline does not imply a good estimate: Q2 and C1 finish the warm-budget run with zero successes. Budget timing excludes query parsing, constant encoding and initial walk-order construction, so it understates full planning overhead. Deadline checks are cooperative; one disk operation can overrun the cap.

Measured total of per-query medians:

| Same-session raw-ID mode | LUBM-100, 14 queries | WatDiv100, 20 queries |
|---|---:|---:|
| WCOJ disabled | 340.714 ms | 97.216 ms |
| WCOJ enabled (current cyclic-query policy) | 161.856 ms | 96.798 ms |
| Extra fixed-work sampling, sum of query medians | 23.107 ms | 12.608 ms |

The TSV column `leapfrog_us` means **WCOJ enabled**, not a forced leapfrog on every query; acyclic queries retain the existing binary path. The source now calls these columns `wcoj_off_us`/`wcoj_on_us`; raw records retain their original emitted headers. Q2 is 184.370 ms with WCOJ off against 3.295 ms on. Q9 is 59.858 versus 61.405 ms, a small difference in one session, not evidence for a new switch. Choosing binary from a false-zero sample could destroy the existing Q2 advantage. A per-query oracle selecting the faster measured mode would be post hoc and is deliberately not implemented or presented as a deployable chooser.

**Decision:** keep the current static/cyclic routing. The acceptance criterion of a reliable improvement without query-total regression is not established; there is no automatic sampled-cost chooser to merge. Reusable rank reads and the explicit research estimator can be reviewed separately. A future candidate needs lower-variance sampling (for example exact suffix evaluation), cheaper random access, and calibration/evaluation on separate queries before it can route production requests.

## Implementation and correctness

`Snapshot::row_at(perm, prefix, rank)` returns a row in key order. Memory and immutable disk snapshots locate the indexed row; disk uses the existing checksummed `DiskIndex::get`. The default implementation scans the visible snapshot, so overlay additions/deletions stay correct. `has_fast_rank` defaults to false, and sampling refuses that slow fallback; no hidden full overlay scan is charged as a cheap probe.

`opt::sampling::estimate_walks` fixes the order before drawing a sample. At each pattern it picks a uniform rank among the exact prefix matches, multiplies the weight by that conditional degree, and rejects inconsistent repeated-variable paths. Failed paths contribute zero. The complete fixed-work inverse-probability estimator follows [Wander Join (SIGMOD 2016)](https://users.cs.utah.edu/~lifeifei/papers/wanderjoin.pdf), DOI 10.1145/2882903.2915235. The PRNG uses rejection to avoid modulo bias. Storage failures retain their original I/O error and abort the probe. Count/rank disagreement is an error, not zero cardinality.

Partial runs are discarded on timeout/probe exhaustion. This avoids using only the fast completed walks, but **conditioning an entire experiment on meeting a time limit can still introduce selection bias**; no unbiased-cohort or confidence-coverage claim is made for gated runs. Standard errors are descriptive, especially with few successful walks. Zero observed successes never authorizes an empty query result.

All 34 queries have identical full sorted ID multisets between the two engine modes on this same snapshot, on each repetition. Counts independently match saved round-4 references. These tests do not rerun Jena: the external reference counts and prior all-six-system verification come from round 4. The full `cargo test --all-targets -j 1` passes, including six new tests for indexed ranks in every permutation/prefix, repeated variables and bag multiplicity, storage failures, budget aborts, and visible overlay rank semantics.

## Reproduction and provenance

Source used for the measurements: `17d8be2`; binary and query SHA-256 plus snapshot manifest hashes are in `provenance.json`. The final commit only clarifies comments/headers and adds this readout; it does not change the measured algorithm. Snapshots were opened read-only from the lead's `bench/tools/rdfdb-disk-{lubm100,watdiv100}` directories. Raw TSV and logs are preserved unchanged.

With the shared-machine benchmark lock acquired:

```sh
cargo test --all-targets -j 1
cargo build --release --example sampling_probe -j 1
target/release/examples/sampling_probe /path/to/rdfdb-disk-lubm100 bench/lubm lubm100.tsv
target/release/examples/sampling_probe /path/to/rdfdb-disk-watdiv100 bench/watdiv watdiv100.tsv
```

Offline summaries and round-4 count checks:

```sh
python3 docs/runtime-sampling/analyze.py > docs/runtime-sampling/summary.json
```

The harness accepts only pure SELECT projection/BGP joins and errors on unsupported modifiers. This prevents reporting a join estimate against an unrelated DISTINCT, FILTER, aggregate or LIMIT result count. There are no new dependencies, background workers, or requests to a live service.
