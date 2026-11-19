# Matched disk-backed aggregate benchmark: 72 labels, two order passes

rdfdb has the lowest labeled total in both order passes of this curated COUNT, COUNT(DISTINCT) and GROUP BY workload. All 1,152 warmup and measured responses pass the preregistered COUNT-compatible comparison with Jena. These observations apply to this workload and these configurations; they do not establish general superiority or full SPARQL conformance.

## Results

Times below retain pass 1 / pass 2 separately. Each engine has 72 measured requests per pass; a separate complete dataset pass is excluded as warmup. The 72 labels contain 68 distinct (dataset, exact query-text hash) pairs. The unique-hash column follows the prespecified rule: average duplicate-label latencies within each hash group and phase, then sum those means. It is not a newly timed deduplicated workload. Primary comparisons retain RDF term identity; COUNT-compatible comparisons only normalize declared nonnegative integer COUNT outputs within checked datatype ranges, preserving grouping terms and multiplicities.

| Engine | 72-label total, seconds | 68-hash mean-sum, seconds | Labeled geomean, ms | Primary exact-term scored passes |
|---|---:|---:|---:|---:|
| rdfdb disk | 0.575291 / 0.538254 | 0.568544 / 0.530698 | 3.295 / 3.190 | 72/72 / 72/72 |
| Virtuoso | 4.610540 / 4.499717 | 3.935858 / 3.827064 | 5.801 / 5.844 | 72/72 / 72/72 |
| QLever | 5.105631 / 4.604266 | 4.853593 / 4.364637 | 31.921 / 28.676 | 0/72 / 0/72 |
| DuckDB typed-triples SQL | 73.148910 / 73.591534 | 70.110467 / 70.612306 | 673.890 / 679.931 | 72/72 / 72/72 |

All engines pass 72/72 COUNT-compatible scored results in each pass. QLever passes 0/72 primary exact-term comparisons because its COUNT datatype differs from Jena; the registered gate handles this distinction explicitly. The other three engines pass every primary comparison. Across warmup plus measurement there are 864 primary passes and 1,152 COUNT-compatible passes. No workload request failed, timed out or was discarded. Shutdown exit codes are retained: 137 for containers stopped after completed workloads, 0 for Virtuoso; every container has OOMKilled=false and was removed.

| Dataset / engine | Pass 1 total, seconds | Pass 2 total, seconds |
|---|---:|---:|
| lubm / rdfdb | 0.312124 | 0.305415 |
| lubm / virtuoso | 4.307525 | 4.194123 |
| lubm / qlever | 2.251990 | 2.135650 |
| lubm / duckdb | 48.652597 | 47.181510 |
| watdiv / rdfdb | 0.263167 | 0.232840 |
| watdiv / virtuoso | 0.303015 | 0.305594 |
| watdiv / qlever | 2.853641 | 2.468615 |
| watdiv / duckdb | 24.496313 | 26.410023 |

QLever/rdfdb labeled-total ratios are 8.87× / 8.55×; Virtuoso/rdfdb ratios are 8.01× / 8.36×. The WatDiv totals for rdfdb and Virtuoso are much closer than the combined totals suggest. Per-query observations and group counts are in round1/query-records.csv; do not interpret a total-time ratio as a win on every query.

## Design and resource profiles

The frozen protocol SHA-256 is `394b422c61409e27547ea471293e4406d9d8513f4bee6751762e4dc5d6e937cf`, registered before this timing run and published in ../aggregate72-matched-protocol-v1/. The frozen runtime source was unchanged after outputs. rdfdb source is `9a2f49b652a73466873cacd0a9205098e27d15f6`; its freshly built Linux ARM64 binary SHA-256 is `4194d984202d1ee20d57e0a51f4a29928a908a55963808dbd50c5ccbb6c33a68`. This is a distinct source cohort from earlier BI100M measurements on ef7a778.

All four systems use disk-backed stores, the same existing Linux VM (16,808,177,664 bytes RAM, 10 vCPUs), pinned arm64 images, four-CPU quotas and 12 GiB container memory with no extra swap. A quota does not pin cores. One engine runs at a time. Pass 1 runs LUBM then WatDiv with rdfdb, DuckDB, Virtuoso, QLever; pass 2 reverses both dataset and engine order. There are 16 fresh-process phases, each with one full dataset warmup followed by one measured pass. Query-label order remains fixed.

rdfdb uses --disk, --cache-mb=10240, four Rayon threads and waits for statistics readiness. DuckDB 2.0.0 uses the frozen lossless typed-triples SQL translation, 8GB execution memory and an 8GB spill limit; there are no property tables or numeric indexes. The Python HTTP adapter has no result memoization. SQL translation, planning, execution, fetch and JSON serialization occur inside the timed request. It is a scoped SQL baseline, not a claim about all DuckDB schemas or applications.

QLever uses the pinned protocol image, 11GB query memory, four threads, groupby hash optimization disabled and three 1MB result-cache caps. Before every warmup and measured workload request the harness clears the QLever result cache and verifies five zero counters: all 288 acknowledgements replay correctly. Other engines retain their stated default internal caches. HTTP requests use Cache-Control:no-cache and Accept-Encoding:identity; this does not itself guarantee bypassing every engine cache.

Virtuoso uses the pinned image, four threads, NumberOfBuffers=1360000, MaxDirtyBuffers=1000000, a 600-second query timeout and a large result-row limit. Its prequalified named graph is selected by HTTP default-graph parameter. No credentials are required for these read-only HTTP requests. Reopen/checkpoint may write administrative data in the owned store; therefore source provenance, full graph counts and every result are checked rather than claiming byte-immutable Virtuoso database files. No private database folders or keys are published.

The timer spans HTTP submission until complete body receipt. Validation, cache-control calls and log writes are outside it. These are end-to-end HTTP observations, not engine-only or embedded API timings. OS page caches are not cleared. This shared Mac host already had approximately 20 GiB of swap used before the run; the host-before observation is retained. No immediate end-of-run host snapshot was captured before another owner's work, so no exclusive host-swap delta is claimed. Two order passes are insufficient for a significance claim; retain both and their drift.

## Memory and integrity

The memory metric is container cgroup memory.peak, not process RSS, engine-only execution memory, or per-query peak. It includes startup, full warmup, measured queries, probes and charged file/kernel pages. Maximum phase values within each engine/pass are:

| Engine | Pass 1 / pass 2, GiB |
|---|---:|
| rdfdb | 0.974 / 0.944 |
| virtuoso | 10.375 / 10.367 |
| qlever | 0.291 / 0.290 |
| duckdb | 3.355 / 3.357 |

These unequal actual memory usages reflect different configured engines; the common container ceiling does not equalize cache algorithms or physical working sets. The root scratch guard sampled at two-second intervals: maximum observed allocation was 19.73 GiB, minimum observed filesystem free space 223.32 GiB. Sampling is not a hard instantaneous quota.

The independent post-run audit replayed all 1,152 complete result bags, each body/query/reference hash, all registered runtime and DuckDB module hashes, the Linux binary, source inputs, and immutable rdfdb/DuckDB/QLever store hashes. It verified full graph counts (13,409,395 LUBM; 10,914,314 WatDiv), actual CPU/memory/swap/image settings, cache acknowledgements, no OOM, absent recorded runner processes and no owned benchmark containers. It ran under a separate successfully acquired verification lock after Alex's test window was released. An earlier premature ownership announcement was corrected; no verification ran under his lock.

The frozen curated queries had prior small-fixture/count-pushdown exposure and are not a blind holdout. No official randomized workload mix or broad SOTA result is claimed. This timing cohort does not measure load/build/compaction performance. Preparation, independent Jena references and separate engine qualification reports remain in sibling aggregate72 directories.

## Evidence and replay

round1/verified-summary.json, phase-summary.csv and query-records.csv are derived from retained status and raw responses. postcheck.py is a new post-run analysis script, not part of the frozen runtime; its configured-limit check validates the limit and OOM state without assuming a sampled cgroup peak must never transiently exceed the limit. Recorded shutdown statuses remain visible.

Large response JSON files are losslessly gzip-compressed (mtime zero) for publication. compressed-bodies.json maps every original relative path to the published file and uncompressed SHA-256/length. Decompress those paths into a disposable copy to replay against the original absolute-path environment; this is not a drop-in portable benchmark. No database blobs, credentials, private keys or full inspect environments are included. SHA256SUMS.json hashes every published file except itself. The external input/store identities and historical paths in protocol.json require explicit adaptation before reproduction.
