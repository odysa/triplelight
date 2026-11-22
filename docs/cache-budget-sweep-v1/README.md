# #83 cache-budget sweep: LUBM-1000 and BSBM-100M

Registered in (internal discussion), additions 9a679630) before any run. Binary: main a03774f2, release,
`--features server` (SHA-256 in `binary.sha256`). Machine: macOS arm64, 64 GiB, so the default budget is 16 GiB.
Files: `sweep.py` (one store's sweep) and `run.sh` (the driver: binary, builds, both sweeps; paths are this machine's).
`raw/`: `lubm1000.json` and `bsbm100.json` (every sample, answer key, idle and peak memory, per run), the printed
summaries, `build-*.log` and `verify-*.txt` (`/usr/bin/time -l` and the per-file sizes), `progress.log`, and
`*.server.log` (`/usr/bin/time -l` per server). The end-of-session default run reused the first default run's log
name, so `*-0.server.log` holds the control run; the first run's peak RSS and footprint are kept in the JSON.

## Answer to the registered question

**No budget of 2 GB or less keeps LUBM-1000 within 10% of the default.** At 2 GB its warm geometric mean is 3.6×
the end-of-session default (3.0× the first default run), and at 512 MB it is 4.6×. At 10 GB it is 1.10×. On BSBM-100M,
2 GB is at 0.99× and 1 GB at 1.28×, but this suite reads little data at 100M (63 ms for all 20 queries), so it
barely tests the cache.

**Most of LUBM-1000's peak memory is not the block cache.** Peak RSS is 9.5 GiB at the default budget, and still
6.0 GiB at 512 MB. A server holds 0.21 GiB before its first query, and a 512 MB cache can hold at most about 0.5 GiB,
so at least 5.3 GiB is query-time memory that the cache budget doesn't control. At the default budget the cache's
share is therefore roughly 4 GiB at most. The two queries that return 7.9M
rows, Q6 and Q14, take most of the time (1.3 s each warm) and are the likely holders; #84 profiles them.

## Results

Warm = median of rounds 2–3; "vs control" divides by the default run repeated at the end of the session. Answers matched
the baseline at every budget, for every query.

| Store | Budget | Warm geomean vs control | Slowest queries (warm ms) | Peak RSS | Peak footprint | RSS before queries |
| --- | ---: | ---: | --- | ---: | ---: | ---: |
| LUBM-1000 | default (16 GiB) | 1.21 (first run) | Q6 2020, Q14 2006, Q9 353 | 9.50 GiB | 8.88 GiB | 0.21 GiB |
| | 512 MB | 4.62 | Q6 4185, Q14 3870, Q9 3371 | 6.02 GiB | 5.17 GiB | 0.21 GiB |
| | 1 GB | 4.04 | Q6 3777, Q14 3755, Q9 3280 | 5.62 GiB | 4.59 GiB | 0.21 GiB |
| | 2 GB | 3.62 | Q14 3552, Q6 3505, Q9 2935 | 6.28 GiB | 5.64 GiB | 0.21 GiB |
| | 10 GB | 1.10 | Q6 1973, Q14 1809, Q9 347 | 9.79 GiB | 9.08 GiB | 0.21 GiB |
| | default (control) | 1.00 | Q14 1293, Q6 1289, Q9 272 | 8.96 GiB | 8.47 GiB | 0.21 GiB |
| BSBM-100M | default (16 GiB) | 1.06 (first run) | q010 29, q013 22 | 0.72 GiB | 0.72 GiB | 0.17 GiB |
| | 10 GB | 1.17 | q010 31, q013 23 | 0.72 GiB | 0.72 GiB | 0.17 GiB |
| | 2 GB | 0.99 | q010 25, q013 19 | 0.72 GiB | 0.72 GiB | 0.17 GiB |
| | 1 GB | 1.28 | q010 67, q013 42 | 0.72 GiB | 0.72 GiB | 0.17 GiB |
| | 512 MB | 5.31 | q013 610, q010 574 | 0.54 GiB | 0.54 GiB | 0.17 GiB |
| | default (control) | 1.00 | q010 26, q013 20 | 0.72 GiB | 0.72 GiB | 0.17 GiB |

`summary` in the JSON files also gives each ratio against the first default run, and cold (round 1) ratios.

## Drift and what it means

The first and last default runs differ: LUBM-1000's warm total is 4459 ms first and 2919 ms last (1.21×), BSBM-100M's
1.06×. The first run of each session pays for the operating system's page cache warming up, which continues into
its rounds 2 and 3. So ratios are given against the end control. Cold (round 1) ratios are confounded the same way
(budgets that ran later had a warmer page cache) and aren't used for conclusions.

## Builds (four orders, the new default)

| Store | Input | Build wall time | Peak RSS | Store size |
| --- | ---: | ---: | ---: | ---: |
| LUBM-1000 | 23.9 GB N-Triples | 627 s | 2.30 GB (2.14 GiB) | 3.0 GiB: dictionary 672 MB, SPO 1500 MB, PSO 365 MB, POS 321 MB, OSP 305 MB |
| BSBM-100M | 26.0 GB N-Triples | 633 s | 2.68 GB (2.50 GiB) | 6.1 GiB: dictionary 4620 MB, SPO 1105 MB, PSO 286 MB, POS 253 MB, OSP 299 MB |

Both pass `verify`.

## Limits

- One session per store, three rounds per budget, one machine. The OS page cache stays warm (the files fit in RAM), so
  this measures our block cache over a warm page cache, not cold disk reads.
- The BSBM-100M suite uses the 10M-scale constants and touches little data (63 ms in total), so it says little about
  the cache. LUBM-1000 is the informative workload here.
- Peak RSS can't be split exactly into cache and query memory from these numbers. At 512 MB, 6.0 GiB peak RSS minus
  at most ~0.5 GiB of cache and 0.21 GiB of base process gives a lower bound of about 5.3 GiB of query-time memory.
