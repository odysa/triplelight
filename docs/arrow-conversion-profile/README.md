# Arrow conversion stage profile (task #34, step-4 pre-work)

In an instrumented copy of the Arrow batch reader, the dictionary-lookup stage (`decode_ms`: `ReadSnapshot::with_terms`
over each batch's distinct stored IDs, including the callbacks and owned term copies) accounts for most of the
conversion time on both measured LUBM-1000 queries: per observation, 99.27–99.39% of the instrumented total for Q9 and
87.67–89.85% for Q14. This describes that instrumented copy on this store and machine; it is not a proven bottleneck
of the library reader, and it does not by itself say why decoding costs what it does.

This follows the [Arrow C Stream measurement](../arrow-c-stream-measurement/README.md), whose instrumented
`convert_ms` was one combined stage.

## What was measured

- **Control pass:** the unchanged library `triplelight_c::arrow::IdBatchReader` over a freshly computed `query_ids`
  result, every batch consumed with the PR #45 consumer work (every cell's validity and field bytes folded into an
  order-independent checksum), each batch dropped; the total runs from reader construction through the explicit
  drop of the reader (its ID result and snapshot).
- **Profiled pass:** `ProfiledReader`, a probe-local copy generated from the frozen library `IdBatchReader::batch`
  source by inserting clock lines only (exact comparison in the packet: `batch-source-comparison.diff`,
  regenerated from the frozen sources and checked byte for byte). Stages: `collect` (slice, filter, sort, dedup),
  `decode` (`with_terms` plus owned copies), `scratch` (computed terms), `build` (builders, struct arrays, batch),
  timed inside `batch`; `consume` and `release` timed at the call site. Intervals do not overlap. Each observation
  records its stage sums and a signed, unclamped remainder against its own total; the remainder includes the reader's
  destruction and the per-batch copies dropped after the build clock. The profiled copy's timings are those of a
  separate instrumented implementation, not of the library reader.
- **Each pass acquires its own read snapshot** of the same unmodified store and recomputes `query_ids` on it; `ids_ms`
  is recorded per pass and is outside both totals. The answer gates and the source and base-tree checks are the
  evidence that both passes saw the same data; the snapshots are not the same object.
- **Warm-query experiment:** VM page cache dropped before each phase, then one untimed control warm-up; then both
  passes in the registered order (r1, r3, r5 control first; r2, r4 profiled first), positions retained. One process
  per phase, Linux arm64 Docker (4 CPUs, 12 GiB, no swap, no network, read-only root, private cgroup), a fresh private
  reflink clone of the LUBM-1000 store with its prepared statistics file.

## Results (5 observations per cell)

| | Q9 (44,190 rows, 3 columns) | Q14 (7,924,765 rows, 1 column) |
|---|---:|---:|
| `decode_ms` share of each observation's profiled total | 99.27–99.39% (median 99.34%) | 87.67–89.85% (median 88.50%) |
| `decode_ms` median [range] | 10,230 [9,242–10,549] | 10,976 [10,396–11,078] |
| `collect_ms` median | 18.6 | 234 |
| `scratch_ms` median | 0.18 | 11.0 |
| `build_ms` median | 16.9 | 335 |
| `consume_ms` median | 10.9 | 549 |
| `release_ms` median | 0.08 | 3.4 |
| signed remainder median | 17.1 | 293 |
| batch-distinct ID occurrences decoded per pass | 121,633 | 7,924,765 |
| `decode_ms` per batch-distinct ID occurrence (median) | about 84.1 µs | about 1.39 µs |

All times are milliseconds unless marked. Stage medians are separately computed medians and are not an additive
partition of any one observation; per-observation stage sums and remainders are in the packet.

- **Per-ID figures** divide the combined lookup-and-copy interval by the sum of per-batch distinct-ID counts, so an
  ID that appears in several batches counts each time. The roughly 60× difference between the two cells does not
  control for term mix, term size or ID locality.
- **Q9 locality (untimed diagnostic pass):** across Q9's six batches, of 121,627 gaps between consecutive sorted
  distinct stored IDs, 3,953 are exactly 1, 12,480 are 2–15, 71,077 are 16–255 and 34,117 are 256–65,535; 114,216 of
  its 117,680 maximal runs of consecutive IDs have length 1. This is consistent with a hypothesis that scattered
  dictionary access raises Q9's per-ID decode cost; this run does not test that hypothesis.
- **Control vs profiled:** the sign of each paired difference (profiled minus control) tracks execution order in this
  cohort: in both cells, every observation taken second exceeds every observation taken first, on both paths
  (paired differences: Q9 +1,042, −982, +984, −969, +1,249 ms; Q14 +751, −882, +850, −817, +866 ms). The difference
  of medians (+926 ms and +622 ms) mixes positions and is not an instrumentation-cost estimate; order and other
  within-phase effects are not separated here.

## Gates (all passed)

Every warm-up and pass matched the frozen PR #45 qualification values exactly (rows, cells, batch count,
order-independent checksum; strict JSON types). Stage times were finite and non-negative; sums and remainders were
arithmetically consistent per observation. The Q9 locality pass matched the same answer values, its per-batch counts
were internally consistent (gap-1 count, run-length feasibility, per-batch cell counts), and its distinct-ID sum
equalled the profiled pass's `distinct_ids_decoded`. Base trees, CURRENT and the statistics file were unchanged; only
`LOCK` and `wal.log` appeared in clones; no OOM; one MemTotal across the cohort; identities, the source comparison
and the qualification manifest were re-verified before and after the run.

## Limitations

- One session, five observations per cell, two queries on one store; descriptive only.
- The macOS host cache behind the VM is not controlled.
- Equal outputs detect answer divergence only; they cannot show the profiled copy follows the same allocation or
  cost path as the library reader. Per-batch clocks perturb the copy.
- Locality is a hypothesis; a controlled comparison is needed before attributing decode cost to it.

## Process and evidence

Plan reviewed by Alex Chi and Jianlin Su (v0–v3). Qualification v1 failed on a gate field path (data correct);
v2 was superseded after review found the two totals ended at different points (the control reader was dropped
inside its total, the profiled reader after it) and three missing locality checks; qualification v3 passed 6/6 with
92/92 offline gate controls. Registration v1 was never run; registration v2 (`bc6ebc65…`) was approved by both and
bound to its Raft message; 10/10 phases passed. Jianlin Su independently audited the saved packet (all 167 packet
hashes, 67 qualification files, ten phase records, 35 observation summaries, five locality records and every summary
value); the audit used saved receipts, not live containers or a rehash of the large store. Its note, script and
recomputed values are in [independent-audit/](independent-audit/README.md); `audit.py` reproduces `AUDIT.json` on
the published packet.

[packet.tar.gz](packet.tar.gz) (verify with [SHA256SUMS](SHA256SUMS)) holds the registration and Raft receipt, every
phase receipt and raw probe output, the qualification, the probe, the extracted reader functions and their comparison,
the raw worktree diff, supervisor, launcher and controls, `summarize.py` and `SUMMARY.json`. The compiled probe and
databases are not included. Receipts keep their original host paths.
