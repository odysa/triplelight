# Matched-VM BI rerun after the query-local memo fix

rdfdb on main `20b257a239c825d547b2e08ca6bfb69faa8a4c7e` takes less time than the plain-triples DuckDB 2.0.0 baseline in both passes of this bounded BSBM BI10M cohort. All 240 requests pass the unchanged primary contract for membership, multiplicities, datatypes, predefined numerical bounds and order; the separate COUNT-equivalence profile also passes.

| Pass | rdfdb, seconds | DuckDB, seconds | DuckDB / rdfdb time |
|---|---:|---:|---:|
| 1 | 14.483 | 113.992 | 7.87× |
| 2 | 14.082 | 112.898 | 8.02× |

Each sum covers 45 individual timed requests after 15 warmup requests. Pass 1 runs rdfdb then DuckDB; pass 2 reverses the order, using fresh serial processes. Timing includes complete HTTP response-body consumption, with JSON parsing, hashing and qualification afterward. These are summed request times, not the official BSBM 50/500-mix throughput score. Two passes are not a statistical significance study or a general SOTA proof.

## Scope and comparison with earlier sessions

This is a two-engine same-session rerun. QLever and Virtuoso are not rerun here; their earlier qualification failures remain in ../matched-vm-v1/. No new four-engine ranking is asserted.

The prior `738f0cb` session recorded rdfdb at 27.231 / 28.540 seconds and DuckDB at 55.319 / 64.523. DuckDB is slower in the present session while rdfdb is faster. Thus the entire cross-round ratio change cannot be assigned solely to the memo optimization. Host and run-state effects remain possible; the comparisons supported directly by this experiment are the paired results above. Runtime settings, graph, SQL, validators and DuckDB native module are unchanged.

## q4 and the remaining work

rdfdb has the lower observed template sum for all eight templates in both passes. In particular, q4 takes 7.322 / 7.074 seconds against DuckDB's 12.431 / 11.877 in this session. q4 still accounts for roughly half of rdfdb's measured time. Values below sum measured requests only, in seconds:

| Template | rdfdb pass 1 | DuckDB pass 1 | rdfdb reverse | DuckDB reverse |
|---|---:|---:|---:|---:|
| q1 | 0.665 | 9.386 | 0.669 | 9.118 |
| q2 | 0.186 | 6.955 | 0.190 | 6.590 |
| q3 | 3.167 | 6.162 | 3.312 | 5.767 |
| q4 | 7.322 | 12.431 | 7.074 | 11.877 |
| q5 | 1.739 | 23.847 | 1.584 | 22.990 |
| q6 | 0.250 | 32.461 | 0.215 | 32.994 |
| q7 | 1.055 | 10.869 | 0.941 | 11.245 |
| q8 | 0.098 | 11.881 | 0.096 | 12.318 |

The full 100M BI and 72 curated aggregate comparisons are still pending. The 100M input/workload is prepared and the query-only Jena reference count is 98,861,444, but agreement with the NT-derived DuckDB import and numerical group verification is not complete.

## Correctness gate before timing

An earlier memo candidate, `1e862e3`, returned 11 rows on a six-row independent fixture because temporary AST addresses could reuse cached state. It was rejected before its BI run; the failure is preserved in ../memo-lifetime-regression-v1/.

Fixed `40ce56e` guards memo slots with owned structural pattern/expression identity. It passes the lifetime fixture and all 60 frozen BI instances, complete bags and 36 ordered sequences before merge. Main `20b257a` differs from that qualified candidate only in README.md, as recorded in `round/qualified-source-equivalence.json`. The Linux binary independently returns the correct six-row Jena bag before this timed cohort. The incorrect candidate's host speed figures are not used here.

## Runtime and resources

All phases run in the same ARM64 Linux VM with four-CPU quotas and 12 GiB container caps. rdfdb uses four Rayon threads and a 10,240 MiB cache target; DuckDB uses four threads, an 8 GB working-memory target and a 6 GB spill limit. These are different resource-accounting mechanisms; quotas are not dedicated physical-core pinning. Readiness and the warmup mix precede measurement, so this is a warmed-query result, not cold-start latency.

The input remains 10,102,032 unique triples with SHA-256 `3a207c6725820036eec87d08dd414090d3548ac2839ca192414dad6e352f680e`. DuckDB uses one lossless lexical triples table and corpus-scoped hand-written SQL for the eight BI templates, without property tables, materialized joins or an answer cache. This is not an optimal-schema or general SPARQL frontend claim.

The pinned Rust builder copies source into the container and verifies all selected hashes before compilation. ELF SHA-256 is `b302f11c49cc2e24d6d2a23d1b7cb3c51f2c358f5f17ee49c5395d2cfa3579d4`, matching both live rdfdb processes. Separate build targets preserve the previous `71ab35c` and `738f0cb` binaries. DuckDB's server reports 2.0.0, and its native-module hashes from the same container environment match both passes and the prior session.

Whole-server peak RSS includes startup, readiness, warmup and measured requests:

| Engine | First pass, GiB | Reverse pass, GiB |
|---|---:|---:|
| rdfdb | 1.388 | 1.398 |
| duckdb | 1.448 | 1.450 |

The 200 ms sampler observes zero named temporary bytes under configured TMPDIR/spill paths. This does not prove zero spill: unlinked, transient and other-location files can be missed. Raw cgroup high-water/limits and process measurements remain available. The host scratch cap is 48 GiB and the free-space floor is 40 GB.

## Audit and evidence

All four clients finish successfully, every request qualifies, no container is OOM-killed, and all owned containers exit before the lock is released. No controller repair, timeout, missing request or timed-sample discard occurs in this rerun. Final checks match the input, read-only DuckDB file, current/preserved binaries, selected source hashes, 60 query hashes, 32 numerical input bindings and COUNT freeze. No old outcome or validator was changed.

The unchanged auditor verifies every raw-body hash, scheduled request, eligibility flag and total with explicit primary scope `rdfdb duckdb`. Its identity matches the previously validated scope-aware version. Larger JSON/body files are losslessly gzip-compressed with raw/stored hashes in `file-manifest.json`; binaries and database blobs are excluded while provenance is retained. Restore a copy and verify its hashes before running:

```sh
python bench/analytical/summarize_http.py --run <restored-round-directory> --primary-profiles rdfdb duckdb --out <new-summary.json>
```
