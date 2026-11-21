# #34 Phase B2: scan-resistant dictionary block cache (design draft v1)

Owner: Alex (storage). Executor and measurement review: Kaiming. Audit: Jianlin.
Status: FROZEN v1 (Kaiming approved #34 msg 1d9b7494; merge-gate details pinned per that review). Nothing has been built or run.

## Hypothesis (from A2, untested)

`LazyIndex::block` charges each cached block `raw bytes + key bytes + 128 × rows`. That's about 39 KB for a
256-row Id-order block whose raw bytes are about 4.8 KB. Both dictionary orders share one `DictionaryCache` of
16 shards × budget/3/16, which is about 1.34 / 2.0 / 2.67 GB at 1× / 1.5× / 2×. Q9's per-pass working set is
about 54,231 blocks ≈ 2.1 GB charged. Eviction is FIFO: a hit doesn't reorder. A cyclic pass larger than
capacity therefore misses on every access. That matches A2's all-miss / all-hit steps (Q9 between 1.5× and 2×,
Q14 between 1× and 1.5×).

## Step 1: confirm the hypothesis (probe-only, no behavior change)

At the end of each A2 window, read out per shard: `used` bytes, cached block count, and the rows and raw bytes of
each cached block. Registered prediction: the mean charge per Q9 Id-order block is 35–43 KB. Q14's mean
charge lies between 1.34 GB / 50,916 ≈ 26 KB and 2.0 GB / 50,916 ≈ 39 KB, consistent with its flip between 1×
and 1.5×. If the charge
is outside these ranges, the hypothesis is wrong and Step 2 is re-planned before any code.

## Step 2: candidate policy, with the budget unchanged

**Reference-bit sampled eviction with probabilistic admission.** A hit sets the block's reference bit, and nothing
is reordered. On a miss into a full shard, the block is admitted with probability p = 1/4. To make room, the
shard samples 8 resident blocks, clears the reference bits of those it samples, and evicts an unreferenced one
(the last sample if all were referenced), repeating until the block fits. A non-admitted miss returns the decoded
block uncached. A shard with room admits every miss. Each shard has its own fixed-seed xorshift generator.

v0 used plain random eviction. Kaiming's review pointed out that it gives a small hot set no protection, whereas
LRU does. The reference bit restores that protection, as the skewed traces below show.

### Simulation (`sim2.py`: 16 shards, equal-size blocks, block % 16 sharding)

Cyclic trace, W = 54,231 blocks. Each phase is a warm-up pass then two scored passes, and the hit rate is shown
as warm-up / position 1 / position 2:

| capacity / working set | FIFO | LRU | random + admit 1/4 | ref-bit + admit 1/4 |
|---|---|---|---|---|
| 0.64 (Q9 at 1×) | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0.62 / 0.61 | 0 / 0.61 / 0.60 |
| 0.95 (Q9 at 1.5×) | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0.95 / 0.95 | 0 / 0.95 / 0.95 |
| 1.27 (Q9 at 2×) | 0 / 1 / 1 | 0 / 1 / 1 | 0 / 1 / 1 | 0 / 1 / 1 |

Skewed traces, with a 20,000-block cache and the first 200,000 accesses as warm-up:

| trace | FIFO | LRU | random + admit 1/4 | ref-bit + admit 1/4 |
|---|---:|---:|---:|---:|
| Zipf s = 1.0 over 200,000 blocks | 0.716 | 0.751 | 0.718 | 0.744 |
| hot set of 2,000 blocks (50% of accesses) + scan over 200,000 | 0.450 | 0.499 | 0.457 | 0.497 |

So the ref-bit policy stays within 0.01 of LRU on both skewed traces and reaches about 0.60 where LRU gets 0.

**Why p = 1/4.** It trades steady-state hit rate on loops against recovery after a working-set change. In the
v0 simulation (cyclic, capacity / working set 0.64), steady hit rates were 0.38, 0.53, 0.60 and 0.63 for
p = 1, 1/2, 1/4 and 1/10. After a complete working-set change, p = 1/4 recovered to 0.18 / 0.28 / 0.35 / 0.40
over the next passes, and p = 1/10 to 0.09 / 0.16 / 0.21 / 0.26. 1/4 keeps most of the loop gain and still
recovers in a few passes. It was chosen from these simulated trade-offs, not tuned to Q9 or Q14.

**Determinism.** The fixed seeds make runs reproducible only when each shard sees its accesses in a fixed order,
as in A2's single-reader conversion window. Concurrent queries or a parallel `with_ids` break that, so the
reproducibility claim covers single-reader runs only. Gates use hits + misses, not exact hit counts across
processes.

## Registered predictions for the scored run (A2 schedule, cells and budgets reused)

Hit rates are for the conversion window's slot 7 and are registered **per pass position**. Position 1 is the
first scored pass after warm-up, and position 2 the second, as rotated.

- Q9 at 1×: 0.50–0.64 at both positions. Q9 at 1.5×: at least 0.90 at both. At 2×: all hits, as in A2.
- Q14 at 1×: at least 0.60 at both positions. At 1.5× and 2×: all hits, as in A2.
- A position effect, meaning positions 1 and 2 differ by more than 0.05 in any cell, is reported as a finding
  and doesn't fail the run.
- `decode_ms` falls roughly in proportion to the misses removed: Q9 at 1× by about 40–60%.
- Memory stays flat, since the budget is unchanged: cgroup `memory.stat` anonymous memory at the end of each window is within ±5% of A2's at each budget. The cgroup peak includes file cache, so it is reported but not gated.
- hits + misses per cell and window equals A2's exactly.
- **Query window:** this cache also serves slot 6, and slot 7 during query evaluation, so admission changes the
  query window too. The query-window slot totals and `ids_ms` are registered and reported alongside, as in A2.
  The prediction is that query-window hits don't fall below A2's at any budget.

## Gates

- Answers and Arrow output are byte-identical to A2 at every budget.
- Existing dictionary and cache unit tests pass. New tests: a full shard admits at the configured rate under a
  seeded generator, eviction never exceeds the budget, a referenced block survives one eviction sample sweep,
  and `clear_cache` resets the generator and the reference bits.
- **Merge gate on standard workloads (pinned):** BSBM BI10M and LUBM, using the embedded Node harness and the
  frozen workloads and references. The order is same-session and interleaved, old / new / new / old, repeated
  twice, so each binary gets 4 fresh-process phases, with the old binary being main at the B2 base commit. The
  statistic is, per query family, the median of the 4 summed scored-pass times for each binary. The gate fails if
  any family's new median exceeds its old median by more than 5% and by more than 50 ms in absolute terms. All
  answers must match the audited references in every phase.
  - **Outside load:** before and after every phase, `docker ps` and per-container CPU for any container not ours is
    recorded. If outside containers are present with CPU above 5% at any snapshot, the gate doesn't decide.
    Instead it reruns once with the repetitions doubled to 4 (8 phases per binary). If the rerun is also contended,
    the merge waits for a quiet window. A contended run can therefore neither pass nor block the merge on its own.
- Timings are reported as taken on a shared machine if the outside batch is present. The counters are the primary
  result.

## Out of scope

Caching compressed blocks (decode on every hit, plus the verified-bytes questions from B3), changing the
budget split, and charging accounting closer to resident memory. Each is a separate, later candidate.

## Erratum (2026-11-21, after freeze `b2db0624…`)

Pool capacities are `budget_mb × 1024 × 1024 / 3` bytes: 1.40 / 2.10 / 2.80 GB (decimal) at 1× / 1.5× / 2×. The
"1.34 / 2.0 / 2.67 GB" figures above treated MB as 10⁶. Corrected Step 1 bounds: Q9's charged working set
(~2.1 GB) is at the 1.5× capacity, so thrashing at 1.5× needs shard skew or slightly larger blocks, which Step 1
reports per shard. Q14's mean charge per block must lie between 27.5 KB and 41 KB. Step 2's predictions stand,
apart from capacity / working-set ratios that now read 0.67 / 1.0 / 1.33 for Q9.

## Amendment 1 (2026-11-21, before any gate run; Kaiming #34 msg 2ef2fcab)

Outside-load detection samples the CPU of every non-ours container every 10 s throughout each phase, not only
before and after it. The 5% rule applies to the maximum sampled value over the phase.

## Amendment 2 (2026-11-21, before Step 1 runs; Kaiming #34 msg 6ca46cf4)

Registered Step 1 bounds, assuming no shard skew. Q9 thrashes at 1.5× and fits at 2×, so its mean charge per
Id-order block lies between 2.10 GB / 54,231 ≈ 38.7 KB and 2.80 GB / 54,231 ≈ 51.6 KB. Q14's lies between
27.5 KB and 41 KB. If Q9's measured charge falls below 38.7 KB, the named alternative is shard skew: the busiest
shard's charged working set exceeds its 1/16 share. Step 1 reports each shard's occupancy for that check. This
supersedes the earlier 35–43 KB range.

## Amendment 3 (2026-11-21, after gate v1 ran, before its verdict was read; Kaiming #34 msg 6051e32e)

The gate evaluator was written after gate v1's phases ran. Its first version split each sampler line into only
three tab fields, so it saw only the first container on each line and hid a 13.2% sample. It was fixed to split at
most twice (`gate-v1/evaluate-v0-to-v1.diff`) before the verdict was read. The fixed evaluator is frozen as
`evaluate-frozen-v1.py` for every later gate run. `gate-v1/control_sampler_parse.py` checks that a 13% sample in a
non-first field trips the rule, that a quiet line doesn't, and that a failed sample is unavailable. Gate v1's output
is kept unchanged. Our own long-running services (`raft-review`, `docker-rdf4j-1`) count as outside load under the
rule. If the REPS=4 rerun is also contended, the frequency of such samples is recorded and the merge waits for a
quiet window, as frozen.

## Amendment 4 (2026-11-21, prospective, before gate v3; Kaiming #34 28f329ce, Jianlin 59a8c4d3)

A gate phase is classified by `contended.py`. Its coverage window is [start − 12 s, end + 12 s], the sampler period.
The phase is contended if any available sample in the window has a container above 5% CPU, unclassified if the window
holds no available sample, and clean otherwise. A contended or unclassified phase is invalidated (kept as
`x<slot>-…-invalidN`) and immediately rerun in the same slot, with the same binary and workload, before the next slot
starts. The old/new/new/old order is kept. Each slot gets at most 2 replacements. If a slot exhausts them, the gate
stops and the merge waits for a quiet window (frozen fallback). The gate decides on the final, clean attempt of every
slot with the frozen rule (per family, median of 4 per binary; fail if > 5% and > 50 ms), evaluated by
`evaluate-v3.py`. An answer, completion or identity failure in any attempt, invalidated ones included, is a gate
failure. Reported, not deciding: replacements per binary, flagged if unequal; and a sensitivity verdict computed on
the first attempt of every slot, so both binaries keep their original balance.

## Amendment 5 (2026-11-21, prospective, before gate v4; Kaiming #34 ca30c9da, Jianlin 11b8bb17)

Gate v3's FAIL stands and stays in the evidence (BI q3 +11.9% / +68 ms; replacements old 8, new 2). Two
diagnostics followed. The q3 trace counters were identical for old and new: q3 is all hits, 30k–136k per instance,
with 0 misses. The same-process hit-path microbenchmark gave old 25.63 ns vs new 24.58 ns per lookup. Neither
explains the end-to-end v3 result. Gate v4 is the qualifying gate. It differs from v3 only as follows.

- **Host sampler** (`host-sampler.sh`): the raw host process table (pid, ppid, %CPU, command) every 5 s, all raw
  samples kept.
- **Ownership** (`host_outside.py`): ours is the gate script's process tree, meaning descendants of the PID recorded
  in `ours.pids` at gate start, resolved by ppid in each sample. Every OrbStack process is excluded, since the
  container VM's load is classified by the Docker sampler, per container. `ps` itself is excluded. Everything else
  is outside.
- **Host threshold:** outside host CPU above 170% of one core. That's the idle baseline p95 of 119.1% plus a
  50-point margin. The baseline was 5 min, 60 samples, lock held with nothing of ours running, 2026-11-21
  07:55–08:00Z: min 23.8, p50 61.2, p95 119.1, max 151.7. The raw data is in `host-baseline/`.
- **Classification** (`contended_v4.py`): a phase is contended if either sampler fires inside its window.
  It's unclassified if either sampler has no available sample in the window. Otherwise it's clean.
- **Window:** [max(start − 5 s, after + 1 s), end + 12 s], where `after` is the timestamp of the sample that
  invalidated the slot's previous attempt (empty on a first attempt). A retry therefore cannot re-catch the spike
  that invalidated its predecessor.
- **Controls:** `control_v4.py` has 10 negative and positive controls covering the edge cases. These include the
  invalidating spike being excluded by `after`, the same spike being caught without `after` (the v3 weakness), a
  new spike after it still invalidating, our own tree and the VM not counting, and the unclassified cases.
- **Repetitions:** REPS = 6 (12 phases per binary), old/new/new/old, with in-place replacement (≤ 2 per slot) as
  in Amendment 4. The decision rule and threshold are unchanged (per family, median per binary; fail if > 5% and
  > 50 ms). The answer-failure rule and sensitivity line are as in v3. Evaluated by `evaluate-v4.py`, which is
  `evaluate-v3.py` plus invalidation counts per sampler.
- **Non-decision:** if v4 can't decide (a slot exhausts its replacements, or a final attempt is unclassified), the
  firing counts per sampler are reported as a measured finding that this host can't run a ±5% end-to-end gate.
  An alternative method (for example in-process CPU time) is then agreed in advance. There is no further rerun of
  this gate.
- **Locking:** `run-gate-v4-locked.sh` takes `/tmp/rdf-bench.lock` before any step and releases it at the end.

## Amendment 6 (2026-11-21, prospective; Jianlin #34 3e6d986a, 7cb519fe; Kaiming 951c570a, 1ad87882) — CPU-time gate

Gate v4 gave no decision for its run and protocol (slot 11 exhausted replacements; the Docker sampler fired on
5 of 31 samples, the host sampler on 2 of 73). B2 is unqualified, and v3's failure stays in the evidence. The
qualifying method from here on is a CPU-time gate, which replaces the wall-time decision.

- **Processes:** one fresh single-engine Node process per full schedule (BSBM BI10M and LUBM-100 agg72, disk mode,
  same bases, private roots), in old/new/new/old process order, REPS = 6 (12 processes per label per workload).
  Builds are the same as before: old is dda13fb5 and new is cf6ad816. No process ever has both engines resident.
- **Measurement:** a diagnostic copy of the frozen Node app (`/tmp/b2-cpu-app`, diff `cpu-app-run.js.diff`) adds
  `process.cpuUsage()` around each request. That is whole-process user + system CPU across all threads, native
  query workers included. Wall time is still recorded. Queries, references and answer handling are unchanged.
- **Warm-up:** the harness's existing warm-up requests in each process are discarded, as before. Base files are
  shared, so page-cache order effects are handled by the old/new/new/old order and reported as order-split medians
  (old-then-new and new-then-old halves of each quad).
- **Decision unit (unchanged rule):** per process, per family, the sum of scored-request CPU ms. Per family, the
  median over each label's processes. FAIL if new exceeds old by more than 5% and more than 50 ms. An answer or
  completion failure in any process also fails. Reducer: `evaluate-cpu.py`.
- **A/A null run first:** the same harness with old under both labels (`MODE=aa`). It must show |new/old − 1| ≤ 2%
  for every family whose median is at least 50 ms CPU, the only families that can trip the 50 ms floor. If it
  fails, the B2 gate isn't run on this method, and that is reported as a measured finding.
- **Descriptive only:** the wall-time ratio, the Docker and host samplers (still recorded) and the order-split
  medians. Sampler data invalidates nothing.
- **What CPU time doesn't remove:** whole-process CPU still includes cache, memory-bandwidth and frequency
  effects of outside load, and it charges any background work in the process (for example the statistics loader)
  to the request that overlaps it. Persisted statistics aren't present for these bases, so each process runs its
  background statistics scan. The harness waits for it before the first request (`stats_ready`), so it falls
  outside the timed requests.
- **Locking:** `run-cpu-gate-locked.sh aa|ab` takes the lock before any step.

### Amendment 6 revisions (2026-11-21, before any CPU-gate run; Kaiming a8c0215a, Jianlin 746b1591)

- **Persisted statistics, no scan.** Every fresh root gets the prepared statistics sidecar (BI10M
  `e460b114…`, LUBM-100 `efb01547…`). These are byte-identical to the files every earlier gate process wrote for
  these bases, under both builds. So no statistics scan runs, and the scan can't fill the dictionary cache that B2
  changes. The Node binding doesn't expose `stats_origin`, so the persisted path is evidenced and bound per process
  instead: the sidecar's SHA-256 must equal the prepared file's, and its size, mtime and hash must be unchanged
  after the process exits (a scan would rewrite it). A process that fails this check fails the gate.
- **Full warm-up.** BI10M's frozen warm-up uses other parameter instances (0 of the 45 scored labels), while
  agg72's covers all 30. So the CPU app runs one untimed, discarded pass over every scored request before the
  frozen schedule, in both builds (`meta.prewarm_scored`; a missing or failed pre-warm fails the gate). The scored
  requests are therefore measured on a warmed cache.
- **A/A band.** The ±2% test applies to every family whose median reaches 50 ms CPU under either label.
- **Stated limits, added.** Whole-process CPU includes V8 compiler and GC threads. Their cost is recorded as part
  of the total and isn't assumed to be identical between builds.
- **Startup CPU receipt** (Kaiming 84811040, Jianlin a589e424). The CPU app records whole-process
  `process.cpuUsage()` and wall time from process start to `stats_ready`, before the count query and the pre-warm
  (`meta.engine_info.cpu_ms_to_stats_ready`, `wall_ms_to_stats_ready`). This corroborates the persisted path: a
  BI10M statistics scan costs seconds of CPU, while loading the sidecar costs milliseconds. A process above 500 ms of
  CPU before `stats_ready` fails the gate. The app diff (`run.js` + `engines.js` vs the frozen app) is
  `cpu-app-run.js.diff`.

## Amendment 7 (2026-11-21, prospective) — Gate D, deterministic qualification (Kaiming 33733518, 60b61ef8; Jianlin 2da7c979)

The CPU-time A/A null run failed (BI q3 0.969 and q6 1.027 against itself, outside ±2%; one process at 535 ms of
startup CPU against a limit I set without a baseline). So the CPU A/B gate wasn't run. End-to-end timing for B2 is
reported as unresolved on this host, and v3's failure, v4's no-decision and the failed A/A all stay in the evidence.
B2 qualifies on deterministic evidence instead.

**D1 – counters on every request** (`gate-d.sh`, reducer `gate-d-eval.py`). Every BI10M query file (60) and every
LUBM-100 agg72 query file (30) runs in a fresh single-reader probe process: `RAYON_NUM_THREADS=1`, tracing on,
default budget 4007 MiB, the prepared statistics sidecar, and `stats_origin` required to be Persisted. Each runs
old, new, new, old. Old is the FIFO probe (701f4adc + probe), new is cf6ad816 + probe + policy counters (worktree
diff `gate-d-probe-worktree.diff`). The vector per process is each pass (control, profiled) × window (query,
conversion) × slot (6, 7) × (hits, misses, evictions). To pass:
- answer checksums are equal across all four processes;
- repeated processes of a build have identical vectors;
- per pass/window/slot, hits + misses are equal for old and new;
- per family (BI query family; LUBM query), new's misses ≤ old's for each window and slot separately.
Every request whose misses rise anywhere is reported even if its family improves. Each request is measured cold
in its own process, so this checks policy decisions and lookups, not cumulative cross-request cache state.

**D2 – policy work.** New's process-wide counters (sweep samples, eviction sweeps, rejected admissions, admissions
into full shards) are reported per request and family beside the misses saved. Old does FIFO evictions with no
sampling.

**D3 – miss-path cost** (`b2_miss_path`, ignored test on `feature/dict-cache-admission`). Both policies are driven
through the same stream of misses into a full shard of 3,400 blocks, in 10 interleaved rounds. Old is the
verbatim pre-B2 FIFO path; new is `Cache::insert` with the admission coin and 8-sample sweep. Reported: ns per miss,
ns per admission, admission fractions, and sweep steps per admission. Ceiling, frozen: new's added cost per miss is
at most 9,000 ns. That's 5% of about 180,000 ns per miss, where the a38 scope is conversion-window profiled
decode_ms divided by slot-7 misses for Q9 and Q14 at 1×.

**Already in the evidence:** a38, the q3 counters and the hit-path microbenchmark (old 25.63 vs new 24.58 ns).
Gate D passes when D1 passes and D3 is under its ceiling. D2 is descriptive.

### Amendment 7 addition — D1b, warm schedules (Kaiming e8bf5762)

Cold single requests can't separate the policies while a shard still has room, because both admit every miss then.
So D1b also runs each full frozen schedule in one warm single-reader process: BI10M's 60 requests and agg72/LUBM's
60, in the harness's plan order with warm-ups first (`/tmp/b2-gd-lists/*.txt`). Process order is old/new/new/old,
with `RAYON_NUM_THREADS=1`, the persisted sidecar and 4007 MiB. Each request runs on the control path, and its
per-request counter windows are recorded. Both probe builds gain the same `schedule` mode (diffs
`gate-d-probe-worktree.diff` and `gate-d-old-probe-worktree.diff`). The D1 old probe is rebuilt from the same
worktree, so D1 and D1b use one old binary. The pass rule is D1's, applied per request in schedule order:
identical vectors on repeats, equal checksums and lookups per request, and per family new misses ≤ old for every
window and slot. Requests with rising misses are reported. The final shard fill (min and max `used` against the shard
budget, and whether any shard is full) is reported for both builds. **If no shard fills, the report says plainly
that D1b shows no regression on these workloads but doesn't exercise the policy, and a38 remains the evidence for
full-cache cases.**
