# Exact count pushdown: measured candidate

Task #13. The candidate removes final-row materialization for supported counts, and uses exact prefix metadata for single-pattern counts and groups. **All 72 aggregate cases and the original 34 queries retain identical complete results.** Eleven aggregate cases also match independent Jena queries. This is a branch for review, not a claim that every query or competing database is faster.

## Results

One macOS host session, one system/process at a time under the shared benchmark lock; four Rayon workers, one warmup and five measurements per query. Timings include parsing, evaluation and decoded results, but exclude canonicalization/hashing. OS caches were not cleared. These are API measurements, not HTTP benchmarks. Statistics were built before timing.

| Operator cohort, sum of query medians | Baseline | Candidate v3 | Ratio |
|---|---:|---:|---:|
| LUBM-100: 30 COUNT / DISTINCT / GROUP queries | 2345.795 ms | 903.984 ms | 2.595× |
| WatDiv100: 42 COUNT / DISTINCT / GROUP queries | 306.039 ms | 257.035 ms | 1.191× |
| WatDiv repeat | 308.060 ms | 252.825 ms | 1.218× |

These are curated operator cases, not a representative traffic mix. They comprise a COUNT-star and DISTINCT-first-projected-variable rewrite of each original query, plus four prefix-group cases. No DISTINCT, FILTER, ORDER, LIMIT or other modifier in an original query is silently dropped by the generator.

| Specific aggregate query | Baseline | Candidate | Interpretation |
|---|---:|---:|---|
| COUNT over LUBM Q6's single pattern, 795,970 matches | 48.305 ms | 0.015 ms | Exact range count avoids enumerating matches |
| Group LUBM rdf:type by class, 15 output groups | 199.531 ms | 0.119 ms | Count each prefix run |
| Group LUBM advisors, 60,160 output groups | 1001.131 ms | 779.545 ms | Output size remains substantial |
| COUNT over WatDiv C3's BGP, 429,276 bindings | 55.855 ms | 28.648 ms | About 1.95×; final binary-join ranges are counted |
| COUNT over LUBM Q9's BGP | 54.088 ms | 52.453 ms | Small difference; no robust Q9 speedup claim |

**The original, nonaggregate queries are separate.** Their total medians were 276.651 → 276.925 ms on LUBM and 130.510 → 133.235 ms on WatDiv. The new entry point is only in `eval_group`; these original queries keep the prior execution path. The measurements do not establish statistical equivalence or exclude small regressions.

A remaining caveat is the fallback `COUNT(DISTINCT ?v0)` rewrite of WatDiv F4: 0.346/0.356 ms in two baseline runs against 0.459/0.406 ms in two candidate runs. It does not enter the supported counting path, and the difference is not localized. Keep this small, repeatable adverse observation in the review; the totals must not hide it. No cross-engine aggregate speed ranking, peak-memory improvement, or production non-regression guarantee is claimed.

## Behavior and scope

The gate requires a `Group` directly over a pure BGP, recognized COUNT aggregates only, and one completely unbound input binding. Other algebra retains the ordinary aggregator.

- **One triple pattern:** COUNT-star and non-distinct variable counts use exact prefix counts; DISTINCT-variable counts use distinct-key seeks, or an exact count if the variable is the last key. Unbound variables count as zero.
- **Groups on that pattern:** constants and group keys form a permutation prefix. The executor visits group-key runs and counts their ranges. It returns one zero-count row for an empty global aggregate, and no rows for an empty grouped aggregate. Missing group variables remain unbound.
- **Pure BGP joins:** global COUNT-star and non-distinct variable counts retain the original binary/leapfrog choice and order. Binary joins materialize their ordinary intermediate prefixes but count the final range instead of producing final rows. Cyclic leapfrog counts the final variable's intersection; the first variable uses the same parallel morsels as the ordinary join. Independent components can multiply counts rather than enumerate a Cartesian product.

Grouped joins, distinct projected-variable join counts, repeated-variable patterns, mixed SUM/MIN/etc., FILTER/OPTIONAL/subqueries inside the group, and distinct-star with existential pattern blank nodes fall back. IDs remain opaque RDF-term identities, including distinct lexical forms and datatypes. This code changes neither the dictionary nor the storage format or Snapshot API.

Storage errors follow the existing Storage-error path, never a successful zero aggregate. Count arithmetic is checked at u64; overflow returns an error rather than wrapping. This is not arbitrary-precision counting. Intermediate prefix buffers can still be large, so no general bounded-memory claim follows from avoiding final rows.

The semantic reference is [SPARQL 1.1's aggregate definition](https://www.w3.org/TR/sparql11-query/#defn_aggCount). The implementation deliberately leaves unsupported shapes to the existing evaluator instead of approximating their semantics.

## Verification and rejected candidate

`cargo test --all-targets -j 1` passes **83 tests** at candidate v3. New tests cover single-pattern index-only execution, DISTINCT/unbound/empty cases, grouped counts, deterministic random graph comparisons against materialized bag sizes, repeated patterns and variables, fallback algebra, and count/seek failures. The tests found and fixed an initial unbound-variable handling omission.

The first candidate forced serial leapfrog for join counts. It preserved answers but regressed COUNT versions of Q2/Q8/Q9 and WatDiv C3; it was rejected. Final code preserves the existing join policy and parallelism. Old measurements are retained and explicitly named v1/v2; use v3 for the results above.

Correctness evidence is stronger than comparing output lengths:

- Every baseline/candidate query has five stable full canonical result hashes; variable names, RDF terms, datatypes and duplicate multiplicity participate.
- All 34 COUNT-star rewrites match the saved round-4 external row counts.
- The original 34-query full outputs match across both binaries and their counts match the external reference.
- Eleven fresh Jena checks cover all four new groups, selected DISTINCT cases, and COUNT versions of Q2/Q9/C3. Full Jena bindings match final v3 output (`results/jena-final-checks.json`). LUBM Jena ran against v2 first, then its complete saved bindings were compared to v3; this was not a new Jena run for all 72 cases.

## Reproduction and provenance

Measured baseline is `35c5591`; measured final candidate is **`b4ef97c`**, preserved as tag `task13-count-measured`. Build both with the identical `examples/count_probe.rs` (copy the example into a detached baseline worktree). Both builds used one Cargo job. `provenance.json` records executable/helper/query/snapshot-manifest identities and runtime controls. It is the pinned source, rather than a moving rebased branch, to which these timings and tests apply. Final integration against main `54bbc43` passes **86 tests**, including the upstream projection-header fix plus new COUNT-under-LIMIT/DISTINCT and literal-identity cases (`integration-rebased.log`). The 83-test result above remains specific to the measured tag; the extra integration checks do not change its timing provenance.

With the benchmark lock acquired:

```sh
cargo test --all-targets -j 1
cargo build --release --example count_probe -j 1
python3 docs/count-pushdown/make_cases.py
# Build/copy fixed executables as baseline and candidate-v3 before running.
python3 docs/count-pushdown/run_host.py /path/to/bench/tools /path/to/fixed-binaries
```

`run_host.py` runs final aggregate cases and the original queries serially. The first baseline aggregate runs and the WatDiv repeat use the same probe directly; their exact arguments and filenames follow the saved TSVs. Do not run probes concurrently. `validate_jena.py` starts its own port-3094 Jena process, refuses an occupied port and stops its process group in `finally`; it was used for correctness, never concurrent with performance timing.

Offline reproduction of summaries:

```sh
python3 docs/count-pushdown/analyze.py lubm docs/count-pushdown/results/lubm-baseline.tsv docs/count-pushdown/results/lubm-candidate-v3.tsv
python3 docs/count-pushdown/analyze.py watdiv docs/count-pushdown/results/watdiv-baseline.tsv docs/count-pushdown/results/watdiv-candidate-v3.tsv
python3 docs/count-pushdown/analyze_original.py lubm docs/count-pushdown/results/original-lubm-baseline.tsv docs/count-pushdown/results/original-lubm-candidate-v3.tsv
```

Large JSON binding files are losslessly compressed. `compressed_artifacts.json` records raw and gzip SHA-256 identities; use `gzip -dc` to restore the original JSON. Canonical full-result hashes remain in each TSV. No benchmark service or worker remains running after the handoff.
