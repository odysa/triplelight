# Matched-VM BI rerun after the two join rewrites

rdfdb on main `738f0cb72ae9a343fd2d7941dbbbec0659203143` takes less time than the plain-triples DuckDB 2.0.0 baseline in both ordered passes of this bounded BSBM BI10M cohort. All 240 scheduled requests pass the unchanged primary qualification: complete row membership and multiplicities, datatypes, predefined numeric bounds and order checks. The separately preregistered COUNT profile also passes, but no relaxation is needed for either engine.

| Pass | rdfdb, seconds | DuckDB, seconds | DuckDB / rdfdb time |
|---|---:|---:|---:|
| 1 | 27.231 | 55.319 | 2.03× |
| 2 | 28.540 | 64.523 | 2.26× |

Each sum covers 45 measured requests after 15 warmup requests. Pass 1 runs rdfdb then DuckDB; pass 2 reverses that order with fresh processes. HTTP timing includes complete response-body consumption, with parsing, hashes and qualification outside. All four phase clients exit successfully, all servers exit, no container is OOM-killed, and all final identity checks pass.

This is a **two-engine same-session comparison**, not a new four-engine ranking. QLever and Virtuoso are not rerun here; their earlier qualification failures remain recorded in ../matched-vm-v1/. Two passes are not a statistical significance study or a general SOTA proof. This is also not the official BSBM 50/500-mix throughput procedure.

## Scope of the change

The earlier pinned-main pilot recorded rdfdb at 177.023 / 191.764 seconds and DuckDB at 110.617 / 142.998. Both engines are faster in the present session, so the entire old-to-new elapsed change cannot be assigned to the code changes. The table above compares engines within the current session. Separate host checks established complete bag/sequence preservation for `54bee3a` and `738f0cb` before merge; their elapsed values were not used as a host/VM A/B.

Main now includes the guarded feature-equality BGP rewrite and compatible-mapping hash join. No frozen validator, workload, COUNT rule or numeric bound changed. The input remains 10,102,032 unique triples with SHA-256 `3a207c6725820036eec87d08dd414090d3548ac2839ca192414dad6e352f680e`. DuckDB still uses one lossless lexical triples table and hand-written SQL for these eight templates, with no property tables, materialized joins or answer cache. This is a scoped SQL baseline, not a general SPARQL frontend or optimal-schema claim.

## Where time remains

Values below sum measured requests only, in seconds. q4 consumes roughly 80% of rdfdb's measured total and remains slower than DuckDB in both passes; that is the next profiling target.

| Template | rdfdb pass 1 | DuckDB pass 1 | rdfdb reverse | DuckDB reverse |
|---|---:|---:|---:|---:|
| q1 | 0.467 | 4.625 | 0.579 | 5.192 |
| q2 | 0.151 | 3.224 | 0.168 | 4.187 |
| q3 | 2.410 | 2.920 | 3.072 | 3.691 |
| q4 | 21.811 | 5.905 | 22.236 | 7.560 |
| q5 | 1.324 | 11.164 | 1.422 | 14.449 |
| q6 | 0.202 | 16.103 | 0.202 | 17.022 |
| q7 | 0.792 | 5.348 | 0.770 | 5.998 |
| q8 | 0.073 | 6.030 | 0.090 | 6.423 |

## Runtime and resources

Both engines run serially in the same ARM64 VM, with four-CPU quotas and 12 GiB container caps. rdfdb uses four Rayon threads and a 10,240 MiB cache target; DuckDB uses four threads, an 8 GB working-memory target and a 6 GB spill limit. The quotas are not dedicated physical-core pinning. Readiness includes graph-count/statistics checks for rdfdb and a registered q1 plus native graph count for DuckDB; measured requests follow the warmup mix. These are warmed query measurements, not cold-start results.

The Linux build copies selected pinned source into the container before compilation, verifies all source hashes there, and uses the same pinned Rust image as the earlier pilot. Its ELF SHA-256 is `8277d341b2a514a3405728530c2bb0de176b3b8b48f11bcbc3f74d4c2b477d23`; each live rdfdb process matches it. A separate target preserves the previous `71ab35c` ELF. DuckDB's server reports version 2.0.0, and native-module hashes collected in the same container environment agree across both passes and the prior pilot.

Observed peak RSS covers the entire server lifetime, including readiness and warmup:

| Engine | First pass, GiB | Reverse pass, GiB |
|---|---:|---:|
| rdfdb | 1.390 | 1.370 |
| duckdb | 1.474 | 1.455 |

The 200 ms named-file sampler observes zero temporary bytes under the configured TMPDIR/spill locations. This does **not** prove zero spill: unlinked, short-lived or other-location files can be missed. Raw cgroup high-water/limits and process RSS are retained separately. The shared-host scratch cap is 48 GiB and the free-space floor is 40 GB.

## Audit and retained evidence

`round/summary.json` audits every raw-body hash, scheduled request, eligibility flag and total. The auditor's primary-profile scope is explicitly `rdfdb duckdb`; its four-engine default correctly rejects a two-engine schedule. A replay of the earlier 12-phase run reproduces its previous summary exactly, apart from the newly explicit scope field. See `round/auditor-validation.json` and the retained auditor source.

Final checks match the source input, read-only DuckDB file, new ELF, preserved earlier ELF and all selected source hashes. All 60 query hashes, 32 numerical input bindings and the COUNT freeze identity also verify. No request fails or is missing, and no controller repair or timed-sample discard occurs in this rerun.

The source/runtime/client commands, raw responses, matrices, resources, lifecycle status and setup provenance are retained here. `file-manifest.json` records raw/stored hashes; larger JSON/body files are losslessly gzip-compressed. Native binaries and database blobs are excluded while their identities are preserved. The 100M input preparation is separate in ../100m-preparation-v1/; full 100M and curated aggregate comparisons are still pending.

To reproduce the summary after restoring compressed files into a separate copy, run:

```sh
python bench/analytical/summarize_http.py --run <restored-round-directory> --primary-profiles rdfdb duckdb --out <new-summary.json>
```

Restore each manifest entry to its original key path, verify the stored hash before decompression and the raw hash/length afterward. Never overwrite the preserved archive or an earlier summary.
