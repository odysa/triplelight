# QLever result-cache check (task #105)

**Result.** The `-c 1MB -e 1MB -E 1MB` flags in `bench/eval/systems.sh` do not turn QLever's result cache off: any result under
the limits is cached, so warm timings of small-result queries (aggregates) measure cache hits. With the cache emptied before
every request (`cmd=clear-cache-complete`, which needs a server started with `-a <token>`: without it the server answers 403),
the same queries take 3 to 640 times longer on the BI q1, q2 and q4 requests (17 to 39 times on q1, 320 to 640 times on q2) and about 90 times longer on LUBM q9. The three
heavier BI q3 windows (selectivity-dependent, 190 to 560 ms) are not cached cheaply and are unchanged.

**Consequence.** Every earlier QLever warm number for small-result queries (harness runs on LUBM, BSBM BI and the
aggregate suites) is invalid as an execution time. The harness now empties the cache before every request (warm-up and timed).

## Method
Same stores as the #105 baseline profile (BSBM-100M, LUBM-1000, built by the harness; mounted via hard links so the server's
query log is written beside the copies and not into the baseline directory), same image by digest (`adfreiburg/qlever@sha256:d7ecda47…`),
same flags as `systems.sh` plus `-a qc`. Docker on the laptop (macOS arm64, 12 GB, 8 CPUs, no swap): the ratios matter, the
absolute times do not transfer to the rental. For each request: 1 warm-up and 3 timed runs, once in `control` mode (cache as the
harness configured it) and once in `cleared` mode (`cmd=clear-cache-complete` before every run, warm-up included). Time to the
last byte, response drained. Medians below. One session, one run per cell: a mechanism check, not a performance claim.

Files: `qcache.py` (runner), `slot.sh` (slot script, local paths replaced by placeholders), `bi.json`, `lubm.json` (all raw timings).

| Request | control median (ms) | cleared median (ms) | ratio |
| --- | --- | --- | --- |
| m000_p00_q1 | 4.4 | 74.3 | 16.9× |
| m000_p01_q3 | 3.2 | 103.0 | 32.2× |
| m000_p02_q2 | 2.7 | 924.3 | 342.3× |
| m000_p03_q2 | 2.4 | 1524.2 | 635.1× |
| m000_p04_q4 | 4.0 | 656.3 | 164.1× |
| m001_p00_q1 | 4.3 | 108.5 | 25.2× |
| m001_p01_q3 | 196.8 | 191.7 | 1.0× |
| m001_p02_q2 | 2.6 | 1097.3 | 422.0× |
| m001_p03_q2 | 2.7 | 1168.0 | 432.6× |
| m001_p04_q4 | 4.1 | 12.4 | 3.0× |
| m002_p00_q1 | 5.7 | 220.6 | 38.7× |
| m002_p01_q3 | 564.1 | 563.4 | 1.0× |
| m002_p02_q2 | 2.5 | 864.3 | 345.7× |
| m002_p03_q2 | 2.7 | 866.4 | 320.9× |
| m002_p04_q4 | 3.7 | 11.0 | 3.0× |
| m003_p00_q1 | 4.7 | 106.4 | 22.6× |
| m003_p01_q3 | 372.9 | 378.7 | 1.0× |
| m003_p02_q2 | 2.3 | 1117.8 | 486.0× |
| m003_p03_q2 | 3.4 | 1450.6 | 426.6× |
| m003_p04_q4 | 3.9 | 11.7 | 3.0× |
| all_q9 | 4.9 | 433.4 | 88.4× |
| distinct_q9 | 4.9 | 441.0 | 90.0× |

## Limits
Laptop Docker; one session; the baseline TripleLight numbers it is compared against were taken outside Docker, so the paired ratios
here are QLever against itself only. Virtuoso and Jena were not measured here (see the plan note).
