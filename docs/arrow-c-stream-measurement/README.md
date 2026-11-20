# Arrow C Stream output (`tl_query_arrow`): descriptive warm-query measurement

For three registered SELECT cells, consuming every cell with identical work, the five-observation ranges of the
public Arrow C Stream path (`tl_query_arrow`, PR #42) and the existing `tl_query` view path overlap in every cell. Overlap is not evidence that the paths are equivalent, and nothing here claims a
speed-up, zero-copy behaviour or a memory change. One registered session on one machine.

## What was measured

- **Paths (public API, timed):** `arrow_api_ms` = `tl_query_arrow` → every batch pulled through the C stream callbacks
  and imported with arrow-rs → every cell's validity and kind/value/datatype/language bytes folded into a checksum →
  each batch and the stream released. `view_api_ms` = `tl_query` → `tl_result_get_view` → the same consumer work over
  every cell → `tl_result_free`. Both include query execution.
- **Instrumented decomposition (a later, separate observation in the same process):** `ids_ms` =
  `ReadSnapshot::query_ids`; `convert_ms` = `IdBatchReader` over that result with the same consumer work, no C stream.
  `convert_ms` measures one combined stage: batch construction, term decoding, allocation, consumer traversal and
  checksum, and batch release. It is not subtracted from, or added to, either public total.
- **Warm-query experiment:** the VM page cache is dropped before each phase, then one untimed warm-up per path (Arrow,
  then view) precedes the timed pair. Timed order per repetition r1–r5: Arrow-first, view-first, Arrow-first,
  view-first, Arrow-first, with every position retained.
- **Environment:** main `b109a64d` (library identical to `73d0cf61`) plus a probe-only diff (the probe example, a
  hidden `tl_db_database` accessor and a `sha1` dev-dependency); Linux arm64 Docker, 4 CPUs, 12 GiB, no swap, no
  network, read-only root, private cgroup; a fresh private reflink clone per phase holding the prepared statistics file.

## Results (medians; [min–max] of 5 observations)

| Cell | Rows | `arrow_api_ms` | `view_api_ms` | `ids_ms` (instr.) | `convert_ms` (instr.) |
|---|---:|---:|---:|---:|---:|
| LUBM-1000 Q9 | 44,190 | 11,102 [10,899–12,276] | 11,798 [10,862–12,270] | 704 [650–705] | 12,299 [12,266–12,525] |
| LUBM-1000 Q14 | 7,924,765 | 13,809 [13,533–14,849] | 13,393 [12,116–14,027] | 1,237 [1,140–1,293] | 15,382 [14,608–15,645] |
| BSBM-100M q010 | 5 | 57.4 [55.1–60.2] | 55.5 [54.3–61.9] | 56.6 [54.0–76.2] | 0.1 [0.1–0.1] |

- **Large cells:** in the instrumented observation, the combined conversion stage takes far longer than building the
  ID result (Q9 about 0.7 s vs 12.3 s; Q14 about 1.2 s vs 15.4 s). This motivates profiling that stage's components
  in step 4. This run does not isolate them: it does not show dictionary decoding, Arrow construction or consumer
  traversal individually to be the bottleneck.
- **Position:** in Q9 and Q14, every second-position observation of a path was slower than every first-position
  observation of the same path (for example Q9 Arrow 10.9–11.1 s first vs 11.9–12.3 s second). Medians therefore mix positions (Arrow is first in
  three of five repetitions, view in two). No cause is established.
- **q010** is a small-result control: 5 rows, one batch; the conversion stage is about 0.1 ms.

Full per-observation values, positions and per-row figures: `SUMMARY.json` in the packet.

## Correctness gates (all passed)

Every warm-up, timed and instrumented observation matched the frozen qualification values exactly (rows, cells,
batch count, order-independent checksum; strict JSON types). q010's ORDER BY was checked on every observation while
consuming rows. An untimed canonical pass on all three paths matched the QLever round-13 bag hashes
(`bench/results/r13-*/qlever.json`; for Q14, Virtuoso's capped answer is not a reference). Base trees, CURRENT and the
statistics file were unchanged; only `LOCK` and `wal.log` appeared in clones. No OOM; one MemTotal across the cohort;
identities and the qualification manifest were re-verified before and after the run.

## Limitations

- One session, five observations per cell; descriptive only, no confidence claim.
- The macOS host cache behind the VM is not controlled.
- The canonical pass validates another execution, not the timed outputs (those are gated by the checksum values).
- The probe is not part of the library; its diff is in the packet.

## Process and evidence

Plan reviewed by Alex Chi and Jianlin Su (v0–v2); qualification v1 and v2 superseded after review fixes (launcher
identity and controls; strict gate types; per-observation q010 order; qualification files frozen and re-verified),
qualification v3 passed 9/9 with 96/96 offline gate controls. Registration v1 was never run; registration v2
(`28fab704…`) was approved by both and bound to its Raft message; 15/15 phases passed. Jianlin Su independently
audited the saved packet (all 15 phase gates, 120 observation checks, 15 canonical q010 order checks, and the exact
summary statistics); the audit used saved receipts, not live containers or a rehash of the large stores. Its
note, script and recomputed values are in [independent-audit/](independent-audit/README.md). The audit covered
packet v1 (237 payload files plus its `PACKET_FILES.json`), which is kept unchanged as
[packet-v1.tar.gz](packet-v1.tar.gz) so the included audit script runs directly on its original input. Packet v2
([packet-v2.tar.gz](packet-v2.tar.gz)) keeps the same 237 payload files byte-identical, adds
`probe-worktree.raw.diff`, `probe-worktree.raw.status` and `PACKAGING-NOTE.md`, and carries a regenerated
`PACKET_FILES.json` listing 240 entries; that manifest itself therefore differs from v1's.

[packet-v2.tar.gz](packet-v2.tar.gz) (verify both packets with [SHA256SUMS](SHA256SUMS)) holds the registration and Raft receipt, every
phase receipt, raw probe output and container inspection, the qualification, the probe source and diffs, supervisor,
launcher and controls, `summarize.py` and `SUMMARY.json`. `PACKAGING-NOTE.md` explains why `probe-worktree.diff` and
`probe-worktree.status` are shell-hook summaries, with the exact raw outputs added beside them; the raw diff matches the
registered worktree-diff hash. Compiled binaries and databases are not included. Receipts keep their original host
paths.
