# Deduplicating parse, default on: budget, eviction and cgroup checks (PR #178)

Measured on `storage/dedup-parse` at 2e4e7b25, the version reviewed first. The merged PR #178 (a69a3de7) changes one
thing after that: the term map is charged its bucket capacity (`capacity × 33` bytes) plus text bytes, instead of
text bytes plus 64 per entry. So it evicts slightly earlier at the same budget, and peak RSS is lower, never higher.
Stores are identical either way (CI's identity tests). macOS arm64, 10 cores, 64 GiB, with the same external containers
as in the ABBA running (`r*.load.txt`, names redacted).

## Checks

- **Lint and tests:** `cargo clippy -D warnings` is clean, and storage tests pass 99/99 with the feature off and
  104/104 with `rdf-12` (`progress.log`).
- **LUBM-1000, four-order external builds, ABA:**

| | r1, default map | r2, 32 MB map per shard | r3, default map |
| --- | --- | --- | --- |
| Wall (s) | 258.4 | 269.4 | 375.7 |
| Peak RSS (GB) | 4.00 | 1.84 | 3.75 |
| Map evictions per shard | 1 | 15 | 1 |

All three stores are identical (`r*.sha256`). The default map on this machine is 256 MB per shard. Evicting fifteen
times instead of once had no time cost measurable within the session's drift (the two default runs differ by 117 s),
and it cut peak RSS by more than half.

- **4 GiB cgroup** (Linux binary in Docker, `--memory=4g --memory-swap=4g`, LUBM-100, four orders): both modes
  finish. The process's peak RSS (VmHWM) is 0.62 GB with dedup and 0.62 GB without, and the cgroup's peak anon memory
  is 0.63 GB against 0.57 GB (`cgroup-rss-{default,off}.log`, sampled every 0.5 s by `cg-rss.sh`). The cgroup's
  `memory.peak` counts page cache and sits at the limit in both modes, so it isn't used.

## Limits

One laptop session with drift and external load. These are mechanism checks, not timing claims; the dedicated
machine repeats the build comparison.
