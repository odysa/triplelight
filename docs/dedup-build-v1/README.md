# Deduplicating parse: identity gates and a LUBM-1000 ABBA (task #91)

Prototype on branch `perf/dedup-91` at 9770ceb3, behind `TRIPLELIGHT_DEDUP_TERMS=1` (default off). The term map holds
`TRIPLELIGHT_DEDUP_MAP_MB` per shard, 256 by default. Each parse shard replaces repeated term texts with shard-local
IDs before the external term sort, so the sort sees each distinct term once per map fill. The design and the
duplication counts behind it are in `docs/parse-phase-v1`. This file is the evidence; the prototype is not merged.
macOS arm64, 10 cores, 64 GiB.

## Identity gates (`gates/`)

One binary, external builds (`--build-external`, checked in each log), every store file hashed:

| Dataset | Orders | Dedup off against on |
| --- | --- | --- |
| LUBM-100 | 4 | IDENTICAL |
| LUBM-100 | 6 | IDENTICAL |
| BSBM-10M | 4 | IDENTICAL |
| BSBM-10M | 6 | IDENTICAL |
| LUBM-100, 1 MB map per shard (constant eviction) | 4 | IDENTICAL to off |

## LUBM-1000 ABBA (23.9 GB, 133.6M triples)

Four-order external builds with dedup off, on, on, off, one at a time, each followed by `verify`. The same four
external containers (`tp-live-swegym-*`, not this project's) ran in every arm (`r*.load.txt`).

| | off r1 | on r2 | on r3 | off r4 |
| --- | --- | --- | --- | --- |
| Wall (s) | 705.05 | 306.09 | 171.02 | 434.96 |
| User / sys CPU (s) | 1025 / 524 | 731 / 159 | 453 / 90 | 1038 / 461 |
| Peak build directory (GB, 2 s sampler) | 43.3 | 21.1 | 19.2 | 51.1 |
| Peak RSS (GB) | 1.34 | 4.05 | 4.56 | 1.97 |

- **Equality:** every file of r2 equals r1, and every file of r3 equals r4.
- **Wall time:** on the ABBA means, dedup on/off is 238.6 s / 570.0 s = **0.42×**. Drift through the session was
  strong (the two off arms are 705 s and 435 s), so quote only the paired ABBA ratio, never a single pair.
- **Mechanism:** sys CPU falls about 4× (less term-sort I/O), user CPU about 1.7×, and the peak build directory about
  halves (47 → 20 GB on average). The peak was the parse-phase shard term sort (`docs/build-compare-orders-v1`).
- **Cost:** peak RSS rises by about 2.5 GB, for the eight shard maps at 256 MB plus hash-table overhead.
- **Eviction was not exercised:** each shard reports `flushes=1`, about 4.1M local IDs for 17.3M triples, which fits
  the map. The 1 MB gate above covers eviction's correctness, not its speed.

## Limits

- One laptop session, two runs per arm, with strong drift and external load. This is a laptop result. The paper's
  build claim (C3) needs the rental's repeated blocks.
- Peak directory size is sampled every 2 s and can miss a shorter peak.
- The page cache was warm for the input after the first build.

## Files

- `run.sh`, `progress.log`, `binary.txt`: the ABBA driver, its log and the binary hash.
- `r*-dedup{0,1}.{build.log,disk,sha256,verify.txt,load.txt}`, per build:
  - `build.log`: `/usr/bin/time -l` output and the per-shard dedup lines;
  - `disk`: peak directory size and lowest free space, KiB;
  - `sha256`: the file hashes;
  - `verify.txt`: the `verify` output;
  - `load.txt`: the containers and top processes before the build.
- `gates/`: the identity-gate driver, its log and the hashes.
