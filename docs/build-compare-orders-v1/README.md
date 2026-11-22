# #88 same-session build comparison: six orders against four

Registered in (internal discussion) before any run. Binary: the #83 sweep binary from main a03774f2
(SHA-256 in `binary.sha256`). macOS arm64, 10 cores, 64 GiB. External builds of `lubm1000.nt` (23.9 GB, 133.6M
triples) and `bsbm100.nt` (26.0 GB), each four times in ABBA order (`--orders=6`, `4`, `4`, `6`), one at a time. Every
store passed `verify` and was deleted after it was measured.

The eighth build (BSBM-100M six-order r4) was stopped by a session time limit on the first attempt and rerun alone
later in the day (16:15 UTC, against 13:44 to 15:44 for the other seven). Two extra LUBM-1000 builds tagged
`-scratch` were added afterwards to diagnose disk use only; their times are not part of the registered comparison.

## Result

**Four orders make stores smaller, not builds reliably faster.**

| | LUBM-1000 six | LUBM-1000 four | BSBM-100M six | BSBM-100M four |
| --- | --- | --- | --- | --- |
| Wall time (s), ABBA runs | 693, 552 | 660, 616 | 779, 522 | 630, 506 |
| Mean wall time (s) | 623 | 638 (1.02×) | 650 | 568 (0.87×) |
| User CPU (s) | 1530, 1154 | 1341, 1278 | 1457, 966 | 1198, 966 |
| Peak RSS (GB) | 2.24, 2.69 | 2.46, 2.39 | 2.78, 2.82 | 2.43, 2.62 |
| Peak build directory (GB) | 44.9, 43.6 | 51.0, 50.5 | 50.6, 45.3 | 45.1, 45.0 |
| Store (GB) | 4.97 | 3.16 (−36%) | 7.94 | 6.56 (−17%) |

- **Time:** builds got faster through the session (LUBM 693 → 660 → 616 → 552 s; BSBM 779 → 630 → 506 s), by more than
  the difference being measured. ABBA cancels a linear trend. On the ABBA means, four orders are 2% slower on LUBM-1000
  and 13% faster on BSBM-100M, whose last build ran in a later slot, about 45 minutes after the third. Neither is a reliable build speed-up on
  this machine.
- **Why the order count barely matters for time:** order builds run in batches of at most three (six orders as 3 + 3,
  four as 3 + 1), so both take two batch waits, and the order phase is only about a quarter of the build (26% in
  #89's profile). The `*.files` timelines show the batches: six orders write SOP, SPO and PSO together, then OSP, OPS and
  POS; four orders write SPO, PSO and POS together, then OSP alone.
- **Memory:** peak RSS is 2.2–2.8 GB for both.
- **Equality:** `dictionary.unified`, SPO, PSO, POS and OSP are byte-identical across all six LUBM-1000 builds
  (the four registered ones and the `-scratch` pair) and all four BSBM-100M builds, six-order and four-order alike.

## Scratch disk: not the order count

The first four-order LUBM-1000 builds peaked about 6 GB higher than the six-order ones (51 GB against 44–45 GB). The
`-scratch` pair, which logs every scratch entry every 2 s, reverses that: six orders peaked at 47.8 GB and four at
42.4 GB. In both, the peak comes **during parsing**, about four minutes into the build. It is the eight parallel
shard term sorters (`.rdf-sort-*-0` to `-7`, 5–7 GB each) holding their runs at the same time. The order phase peaks
far lower (26.2 GB with six orders, 21.8 GB with four). Parsing is the same code for both order sets, so the peak
differences are run-to-run variation in when the shards free their runs. This matches #89's phase log (46.7 GB peak in
the parse phase, about 20 GB in the order phase). Any scratch reduction has to target the shard term sort.

## Limits

- One machine and one session per dataset, two runs per arm, with strong within-session drift. The BSBM six-order
  repeat ran in a later slot, about 45 minutes after the third BSBM build finished.
- The OS page cache was warm for the input file in every build after the first.
- Peak directory size is sampled every 2 s and can miss a shorter peak.

Files: `run.sh` (the driver; paths are this machine's) and, in `raw/`, `progress.log` and per build `*.build.log` (`/usr/bin/time -l`), `*.disk` (peak directory
size and lowest free space, KiB), `*.files` (timeline), `*.verify.txt` and `*.kept.sha256`.
