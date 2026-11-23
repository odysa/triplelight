# Cache-budget sweep on main after #131, plus a same-session timing A/B (gap B4)

Gap B4 in `docs/paper-gaps-v1.md` and plan amendment 1 (A8). This is mechanism evidence from the laptop, not a paper
timing claim. It re-runs the `docs/cache-budget-sweep-v1` (#83) sweep on main a08e3440, after #131 (bounded-window
JSON output). The driver (`run.sh`), suites, budgets and order are unchanged, and the default four-order stores were
rebuilt (LUBM-1000 3.0 GB, BSBM-100M 6.1 GB). Answers match at every budget on both datasets. macOS arm64, 10 cores,
64 GiB.

## Peak RSS (GiB): #83 at a03774f2 against this run

| Budget (MB) | LUBM-1000 | BSBM-100M |
| --- | --- | --- |
| 0 (the default, half of RAM) | 9.50 → 5.60 | 0.72 → 0.72 |
| 512 | 6.02 → 2.39 | 0.54 → 0.57 |
| 1024 | 5.61 → 3.05 | 0.72 → 0.60 |
| 2048 | 6.28 → 3.53 | 0.72 → 0.73 |
| 10240 | 9.79 → 5.34 | 0.72 → 0.72 |
| 0 (control) | 8.96 → 5.66 | 0.72 → 0.72 |

In #83, at least 5.3 GiB of LUBM-1000's peak was query-time memory outside the cache budget. #131 removed about
3.5 to 4 GB from every LUBM-1000 peak, and peak RSS now follows the budget: 2.4 GB at a 512 MB cache. BSBM answers are
small, so nothing changed there.

## Timing: the sweep's own times are not comparable

In the sweep, LUBM-1000 warm times came out 1.8 to 2.5 times slower than in #83 across all queries, tiny ones
included. While it ran, another workload's containers (`tp-live-swegym-*`, not this project's) were running on the
laptop's Docker VM, at about 180% CPU. So the sweep's times are not used.

## Same-session A/B (`ab/`)

To separate load from code, one LUBM-1000 store was built by main 5cfdd0ad, and #83's binary a03774f2 was run
against main on it, in ABBA order (old, main, main, old). Each arm made three warm passes at the default cache
(`--budgets 0,0,0`), and answers matched in every pass. The same external containers ran throughout, and each arm
records them (`ab/r*.load.txt`). Binaries are in `ab/binaries.txt`.

- **Per-query warm medians, main against old:** geometric mean **1.05×**, with 12 of 14 queries within ±11%. q8 is
  **1.30×** (24.6 → 31.9 ms). The two longest queries are 1.00× (q14, about 1.9 s) and 1.04× (q6, about 2.0 s).
- **A slow stretch:** in one main arm (r2), the first two passes totalled 7.1 and 7.4 s, while every other pass
  totalled about 4.2 to 4.5 s. The other main arm (r3) was 4.2 to 4.4 s. That's the same size as the sweep's
  slowdown, and it comes from load, not code.
- **Peak RSS in the same session:** old 8.8 to 9.7 GB, main 6.4 to 7.9 GB.

**Conclusion.** The sweep's slowdown was external load. Main is about 5% slower than a03774f2 on the geometric
mean, which is within noise for most queries. q8's 30% is the one difference to investigate. Laptop timings taken
while other workloads run need ABBA and several passes per arm.

## Limits

- One machine and one session for each run, with other workloads present. The A/B has two arms per binary.
- Peak RSS is the process's maximum resident set from `/usr/bin/time`. Page-cache behaviour is not separated.
- The binaries carry no instrumentation. The A/B's per-query numbers are medians of the warm repetitions in each
  pass, taken across all passes of a binary.

Files:
- `run.sh`, `progress.log`, `binary.sha256`: the sweep driver, its log and the binary hash;
- `{lubm1000,bsbm100}.{txt,json}`: the summaries and per-query times;
- `ab/run.sh`, `ab/progress.log`, `ab/binaries.txt`: the A/B driver, its log and the binary hashes;
- `ab/r*-*.{txt,json}`: each arm's summary and times;
- `ab/r*.load.txt`: the containers and top processes before each arm.
