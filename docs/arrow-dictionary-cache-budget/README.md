# Dictionary cache budget during Arrow conversion (task #34, step-4 Phase A2)

Raising the storage cache budget turned the Id-order dictionary block cache from missing on almost every lookup
during Arrow conversion to hitting on every lookup, at a threshold that differs by query. On LUBM-1000 Q9 the
conversion window records 54,231 misses per pass at 1× and 1.5× the default budget and none at 2×; on Q14, 50,916
misses at 1× and none at 1.5× or 2×. The number of lookups does not change (54,240 for Q9, 51,876 for Q14); only
their outcome does. The profiled dictionary-decode stage falls with the misses. The container memory peak (cgroup,
including file cache) rises with the budget for Q9; for Q14 its median is highest at 1.5×.

This follows the [default-budget counters](../arrow-dictionary-cache-counters/README.md) (Phase A1), which found
near-total misses with evictions about equal to misses, consistent with capacity pressure.

## What was varied

`disk_index::set_cache_budget_mb(B)` before open, with B = 4,007 MiB (the derived default, 1×), 6,011 MiB (1.5×) and
8,014 MiB (2×). The budget is shared: it sets the dictionary caches and the six index caches together. A probe-only
accessor (in the measurement worktree, never merged) reported the capacities the caches were actually constructed
with, and the gates required them to equal the library formulas for each budget (Id-order dictionary
87,534,250 / 131,312,298 / 175,068,501 bytes per shard; see the packet for every capacity). Everything else is the
A1 design: the same probe, control and profiled passes, query and conversion counter windows, trace-on only, prepared
statistics file (no statistics worker), and container profile (Linux arm64, 4 CPUs, 12 GiB, no swap, no network,
read-only root, page cache dropped before each phase). Five repetitions per cell, budget order rotated per
repetition, 30 fresh-process phases, every position retained.

## Results (conversion window, slot 7 = Id-order dictionary, per pass)

In each cell and budget all ten passes (control and profiled) record identical values.

| | Q9 1× | Q9 1.5× | Q9 2× | Q14 1× | Q14 1.5× | Q14 2× |
|---|---:|---:|---:|---:|---:|---:|
| hits | 9 | 9 | 54,240 | 960 | 51,876 | 51,876 |
| miss events | 54,231 | 54,231 | 0 | 50,916 | 0 | 0 |
| evictions | 54,230 | 54,230 | 0 | 50,916 | 0 | 0 |
| bytes returned by `read_at` | 259,835,069 | 259,835,069 | 0 | 243,494,113 | 0 | 0 |
| profiled `decode_ms`, median | 10,241 | 9,721 | 146 | 10,147 | 922 | 906 |
| control conversion pass, median ms | 10,613 | 9,424 | 179 | 11,474 | 1,987 | 1,965 |
| control query IDs + conversion, median ms | 11,543 | 10,182 | 820 | 12,396 | 2,834 | 2,812 |
| container memory peak, median GiB | 2.866 | 3.779 | 4.104 | 3.171 | 3.748 | 3.481 |

- **Units** are as in A1: hits and misses are block-cache lookups, miss events include repeated loads of the same
  block, reads are `read_at` attempts and bytes are bytes returned (possibly from the page cache). None of these is
  physical I/O. The records do not identify the lookup sequence or the distinct blocks touched.
- **Timings** are shared-machine measurements (see below). The budget changes dictionary and index capacities
  together, so timing differences are not attributable to the dictionary cache alone.
- **Memory** is the container cgroup peak, which includes file cache, not resident heap. It rises with the budget
  for Q9; for Q14 the three medians are as listed.
- **Storage reads:** Linux `/proc/self/io` `read_bytes` in the conversion window was zero in 54 of 60 passes and at
  most 640 KiB in the other six. This is Linux storage reads attributed to the process; it cannot establish macOS
  host physical I/O.
- Query-window counters are constant across passes for Q14 and vary across passes for Q9; they are reported per pass
  in `SUMMARY.json`.

## Interpretation (hypothesis, not measured)

The step pattern is consistent with a capacity effect: misses, evictions and decode time fall together once the
budget passes a query-specific threshold. The run does not establish why the cache thrashes at 1× while returning
only about 260 MB per pass against a nominal dictionary share of about 1.4 GB. A source-level hypothesis (Alex Chi,
untested here): each cached block is charged its raw bytes plus key bytes plus 128 bytes per decoded row (about
39 KB for a full Id-order block versus about 4.8 KB read); both dictionary orders share one pool of budget/3; and
eviction is FIFO, so a repeated pass larger than capacity misses on every access. The charged size and per-shard
occupancy are not in these records; Phase B2 measures them before any policy change.

## Shared machine (disclosed)

Another workload outside the benchmark lock (`tp-live-swe-*` containers) was present on the same VM at both
snapshots; the snapshots alone do not show whether it was active in between. Container snapshots before and after the run show four such containers each time, three of them with different names; their
CPU was 0% at both snapshots, and CPU between the snapshots was not recorded. All timings are therefore
shared-machine measurements. The counters are deterministic in every cell.

## Gates (all passed)

A2 inherits every A1 gate (answers, stages and locality against frozen values with strict types; counters exact
non-negative integers, non-decreasing within windows and continuous across windows; trace mode and statistics
origin; base files unchanged; cleanup; no OOM; one MemTotal; identities re-verified before and after) and adds:
the registered budget set before open and equal to the phase's budget, constructed cache capacities equal to the
library formulas, and cumulative `memory.stat` / `/proc/self/io` counters non-decreasing.

## Limitations

- One session, five repetitions per cell, two queries on one store; descriptive only.
- The budget is a single knob for all caches; the dictionary share is not varied independently.
- Peak memory includes page cache; resident cache size is not measured.
- The macOS host cache behind the VM is not controlled.

## Process and evidence

Design reviewed by Jianlin Su (v1) and Alex Chi (v2). Qualification v1 was superseded (it lacked the cumulative
telemetry check) and its registration never run; qualification v2 passed and registration v2 (`a40674ec…`) was
approved by both and bound to its Raft message before the run. 30/30 phases passed. Jianlin Su independently audited
the packet: registration and receipt, 30 phases, 60 passes, 105 answer summaries, 120 counter windows and the 15 Q9
locality records, and recomputed every summary row.

[packet.tar.gz](packet.tar.gz) holds 288 data files plus `PACKET_FILES.json` (their SHA-256 digests). It is a clean
re-archive of the audited packet: the original archive (SHA-256
`9d25da8abcb3fa9f3be4bcdfa06dbf19d09a61ade542d649ad0a363aab092e99`, not published) carried 352 unmanifested macOS
AppleDouble metadata entries alongside the same 288 files, which are unchanged here. Verify with
[SHA256SUMS](SHA256SUMS). [independent-audit/](independent-audit/) holds the auditor's `replay.py` (Python 3.12+,
standard library only) and its output on the original archive (`AUDIT.json`); `AUDIT-clean-packet.json` is the same
replay on this archive and differs only in the AppleDouble count (0 instead of 352):

```bash
python3 -I -B independent-audit/replay.py packet.tar.gz > audit.json
cmp audit.json independent-audit/AUDIT-clean-packet.json
```

Compiled binaries and databases are not included; receipts keep their original host paths.
