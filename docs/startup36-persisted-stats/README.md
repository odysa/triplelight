# Persisted planner statistics: embedded startup comparison (startup36)

With a valid statistics file next to the base, an embedded disk database has cost-based planner statistics as soon
as `Database::open` returns: **0.74–0.91 s** after open start, versus **20.5–25.8 s** when the statistics are
rebuilt by a background scan (about 28× sooner). Whole-process peak memory is lower in every cell because no scan
runs. This compares main just before and after PR #36 (persisted statistics) in one registered session; it is a
descriptive, single-session measurement on one machine, not a general latency claim.

## Conditions

- **A-scan:** main before #36 (`cc93aa03`), fresh clone; always scans.
- **B-reuse:** #36 (`40af216a`), clone holding the prepared `stats-0.v1` (loaded synchronously at open).
- **B-scan:** #36 on a fresh clone without the file: it scans, then writes the file before publishing.

Stores: LUBM-1000 disk (queries Q2, Q9) and BSBM-100M disk (q010). Five fresh-process observations per condition
and cell (45 phases), rotation `[A-scan, B-reuse, B-scan]` shifted per repetition and cell. Linux arm64 containers
(4 CPUs, 12 GiB, no swap, no network, read-only root, private cgroup), VM page cache dropped before every phase.

## Results (medians; registered rule: a B median outside the range of the 5 A-scan observations)

| Cell | Metric | A-scan | B-reuse | B-scan |
|---|---|---:|---:|---:|
| LUBM-1000 Q2 | ready (ms) | 25,760 | 904 (lower) | 25,413 (lower) |
| | first query (ms) | 1,216 | 1,234 | 1,203 (marginally lower) |
| | peak memory (MiB) | 2,233 | 1,009 (lower) | 2,407 |
| LUBM-1000 Q9 | ready (ms) | 25,847 | 906 (lower) | 25,457 (lower) |
| | first query (ms) | 3,930 | 3,927 | 4,109 |
| | peak memory (MiB) | 3,934 | 3,127 (lower) | 3,965 |
| BSBM-100M q010 | ready (ms) | 20,453 | 744 (lower) | 20,111 (lower) |
| | first query (ms) | 4,277 | 4,040 | 4,273 |
| | peak memory (MiB) | 2,624 | 1,699 (lower) | 2,625 (0.1 MiB above A's maximum) |

- "Ready" is open start to the first observed `stats_ready()`; it includes polling and scheduler delay (nominal
  poll interval 1 ms). For B-scan it includes writing the file before publication.
- B-reuse statistics load (`load_ms`): 0.7 ms (LUBM, 62,472-byte file) and 2.9 ms (BSBM, 535,136-byte file).
- B-scan: `scan_ms` 24.5 s / 19.3 s, `persist_ms` 7–8 ms median. B-scan readiness is 1.3–1.7% below A-scan; no
  cause is established, and this is not claimed as a speed-up.
- First-query time shows no measurable change in 5 of the 6 B-versus-A comparisons; Q2 B-scan is marginally lower.
- Peak memory is the container cgroup `memory.peak` for the whole process (open, statistics, query, close), not
  RSS; the q010 B-scan "higher" call is a 0.1 MiB crossing of A's maximum.

Full table with ranges and the residual (post-close) charge: [SUMMARY.txt](SUMMARY.txt).

## Preparation cost (separate, untimed)

Each B-reuse file was produced once by binary B scanning a fresh clone: LUBM-1000 25.4 s to ready (scan 24.5 s,
persist 9.0 ms), BSBM-100M 20.1 s (scan 19.2 s, persist 14.0 ms). These two untimed runs match the measured B-scan
path on the same stores; other stores and circumstances will differ. Preparation is not part of the B-reuse
measurements.

## Limitations

- The macOS host cache behind the VM's virtiofs mount is not controlled; only the VM page cache is dropped.
- The `stats_origin` diagnostic read (`--cfg tl_origin`) exists only in the B build; it is taken after every timed
  boundary, and A prints `unavailable`.
- Compaction-time scanning is not measured; #36 does not change it.
- One session, five observations per condition; comparisons are descriptive with no confidence claim.

## Process and history

- Design, supervisor, probe, launcher and qualification were reviewed by Alex Chi and Jianlin Su before
  registration; Jianlin independently audited the completed attempt (653 packet hashes, all 45 answer, condition
  and resource gates, 15 q010 order checks, all 15 metric rows recomputed). His audit note and script are in the
  packet under `independent-audit/`; it reviewed saved receipts, not live Docker state or a rehash of the original
  multi-GB stores.
- Registration v1 (`33fc9ae9…`) was never bound: the binding check expected a space after the author handle, the
  message rendered a colon, and the detached launcher then stopped at its missing-receipt check before any phase.
  v2 (`ffff5225…`) changed only that check and the output namespace, re-qualified, and is the reported attempt.
- During preparation a failed lock acquisition was followed by a chained command that ran two container selftests,
  one VM cache drop and deleted another agent's lock directory; it was disclosed at once, the affected outputs were
  discarded, and registration and launch were moved into frozen, tested entry points (`register.sh`, `launch.sh`,
  with 19 stubbed controls).

## Evidence

[packet.tar.gz](packet.tar.gz) (verify with [SHA256SUMS](SHA256SUMS)) contains the registration, all 45 phase
receipts, probe outputs, cgroup memory files and inventories, the qualification, prepared files and receipts, the
frozen supervisor/probe/launcher sources, schedule, selftests and launcher controls, the run log, and the
independent audit. Result bodies were byte-identical within each cell across all 15 phases, so one copy per cell is
kept with the hashes (`results/IDENTICAL.json`); every phase receipt records its own result hash. Database files are
not included. Receipts retain the original host mount paths as recorded.
