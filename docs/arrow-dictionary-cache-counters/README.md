# Dictionary block-cache counters during Arrow conversion (task #34, step-4 Phase A1)

At the default cache budget, nearly every dictionary block-cache access made while converting a query result to
Arrow is a miss, and evictions nearly equal misses, despite a prior warm-up query and conversion in the same
process. On LUBM-1000 Q9 the conversion window records 9 hits, 54,231 misses and 54,230 evictions per pass; on Q14,
960 hits, 50,916 misses and 50,916 evictions. This is consistent with capacity pressure in the dictionary block cache
for these working sets and meets the registered condition for testing that hypothesis with a cache-budget sweep
(Phase A2). It does not establish capacity as the cause.

This follows the [conversion stage profile](../arrow-conversion-profile/README.md), where the dictionary-lookup
stage (`with_terms` plus term copies) took most of the instrumented conversion time.

## What was measured

- **Counters:** the storage layer's existing block-cache trace counters (`disk_index::cache_trace_snapshot()`):
  for each of 8 slots, hits, misses, `read_at` attempts, bytes returned and evictions. Slot 7 is the Id-order
  dictionary. The counters are process-global and never reset.
- **Windows:** before/after snapshots immediately around `query_ids` (query window) and around the conversion pass
  (conversion window) separately, on the probe thread; the warm-up is outside both. Each phase runs the a35
  control pass (unchanged library `IdBatchReader`) and profiled pass (`ProfiledReader` copy), each over its own read
  snapshot and recomputed `query_ids`.
- **Trace mode** is process-wide and keyed on the variable's presence. Trace-on phases set
  `TRIPLELIGHT_TRACE_BLOCK_CACHE=1`; trace-off phases are separate processes with the variable absent (checked by
  container inspection). Every phase records `cache_trace_enabled()`, `physical_memory_bytes()` and the default
  budget derived from the library formula (`max(1, physical / 4 / MiB)` MB, not read directly: 4,007 MB here, with a
  16 GiB VM MemTotal and a 12 GiB container limit).
- **No statistics worker:** each phase opens a clone with the prepared statistics file and requires
  `stats_origin() == Persisted`; on that open path `refresh_stats` is not called (`src/embedded.rs` line 93:
  `if !db.load_persisted_stats()? { db.refresh_stats(); }`), so no statistics worker is spawned. Thread counts
  around each window are recorded as a limited diagnostic only.
- **Schedule:** LUBM-1000 Q9 and Q14, 5 repetitions each, each repetition one trace-on and one trace-off phase
  (trace-on first in r1, r3, r5), with the a35 pass rotation inside each phase; 20 fresh-process phases, every
  position retained. Same container profile as before (Linux arm64, 4 CPUs, 12 GiB, no swap, no network, read-only
  root, private cgroup), VM page cache dropped before each phase.

## Results (trace-on, conversion window, slot 7, per pass)

| | Q9 | Q14 |
|---|---:|---:|
| batch-distinct ID occurrences decoded | 121,633 | 7,924,765 |
| block-cache hits | 9 | 960 |
| miss events | 54,231 | 50,916 |
| `read_at` attempts | 54,231 | 50,916 |
| bytes returned by those reads | 259,835,069 | 243,494,113 |
| evictions | 54,230 | 50,916 |
| miss fraction | 99.9834% | 98.1494% |
| ID occurrences per miss event | 2.2429 | 155.6439 |

- **Units:** hits and misses are block-cache lookups. Miss events include repeated loads of the same block; they are
  not counts of distinct blocks. Reads are `read_at` attempts, including retries; bytes are bytes returned, which may
  come from the OS page cache. None of these are physical disk I/O.
- **Determinism:** in each cell all 10 trace-on passes (control and profiled) record identical values. Only slot 7
  moves in the conversion window; the query window touches slots 2, 3 and 6 (Q9) and 3 and 6 (Q14). Every trace-off
  delta is exactly zero.
- **Per-ID cost:** the two cells have a similar number of miss events but very different ID occurrences per miss
  event (about 2.2 vs 156). That is a candidate explanation for the per-ID decode cost difference measured in the
  conversion profile; it is a hypothesis, not a measured distinct-block result.
- **Timing:** trace-on and trace-off pass totals overlap in both cells (median 10,039 vs 9,932 ms for Q9; 12,229 vs
  12,377 ms for Q14). These are observed differences that include order and run-to-run variation, not tracing cost.

## Shared-machine overlap (disclosed)

While this run was in progress, another agent ran three short host-only offline harness checks (no containers or
database work) inside an enclosing window of 11:36:56–11:38:29Z. Phases are reconstructed as the archived empty
`container.log` mtime (created at the `docker start -a` call) plus the receipt's container duration, a
reconstructed host timestamp proxy that excludes preparation. Phases 13, 14 and 15 (Q14 r2-off, r2-on, r3-on)
intersect the window and are flagged. The checks did not directly increment this process's counters, but resource
contention could change execution and cache behaviour, so their indirect effect on both counts and timings of those
phases is unknown. The flagged phases are retained, not discarded.

## Gates (all passed in the reported attempt)

a35 answer, stage and locality gates on every pass (frozen PR #45 values, strict types); counters exact non-negative
integers, 8 × 5 per snapshot, never decreasing within a window and continuous across windows in pass order;
trace-off deltas exactly zero; trace-on conversion windows show slot-7 lookups; observed trace mode equals the
registered mode; statistics origin Persisted; physical memory positive and the derived budget exact; container
environment matches the trace mode; base trees, CURRENT and statistics file unchanged; only `LOCK` and `wal.log`
appear; no OOM; one MemTotal across the cohort; identities and the qualification manifest re-verified before and
after the run.

## Limitations

- One session, five repetitions per cell, two queries on one store; descriptive only.
- The macOS host cache behind the VM is not controlled.
- The default budget is derived from the library formula, not read.
- The counters do not measure distinct blocks; capacity and locality remain hypotheses for Phase A2.

## Process and evidence

Design reviewed by Alex Chi and Jianlin Su (v0–v2). Registration v1 was never run (its gates checked the budget
fields only by type and did not check counter continuity across windows). Attempt v2 stopped at phase 1 of 20 on a
supervisor bug: the scored dispatch did not pass the registered trace mode, so a trace-on phase ran with tracing off;
the trace-mode gate caught it, the remaining 19 phases did not run, and no v2 data is used. After that fix (a
required keyword argument and a frozen full-schedule dispatch selftest), qualification v3 passed 8/8 with 140/140
gate controls; registration v3 (`9616fd02…`) was approved by both and bound to its Raft message; 20/20 phases passed.
Jianlin Su independently audited the saved packet (all 294 packet hashes, twenty phase gates and receipts, forty pass
summaries, zero trace-off deltas and every reported counter value); the audit used saved receipts, not live
containers or a rehash of the large store.

[packet-v1.tar.gz](packet-v1.tar.gz) is the audited packet. [packet-v2.tar.gz](packet-v2.tar.gz) keeps all 294 of its
files byte-identical and adds `summarize-v2.py`, `SUMMARY-v2.json` (the corrected interval reconstruction, identical
counters, timings and overlap flags) and `PACKAGING-NOTE.md`, with a regenerated `PACKET_FILES.json`. Verify both
with [SHA256SUMS](SHA256SUMS). The audit's recomputed values (`INDEPENDENT_AUDIT.json`) and the
script that produced them are in [independent-audit/](independent-audit/), as a record. The script was written to
run inside the auditor's working copy of the packet and reads that review directory, so it is not a portable replay
of the published archives. Compiled binaries and databases are not included; receipts keep their
original host paths.
