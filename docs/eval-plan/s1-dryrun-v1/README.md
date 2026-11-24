# S1 runner, laptop dry run (#92; layout only, no timing claims)

`bench/eval/s1-run.sh lubm100 bench/lubm <out> 1 2` ran on the laptop (macOS) on LUBM-100, with TripleLight only.
There were 2 timed runs per query, and the reference answers came from TripleLight itself, which is enough to check the
output layout. A real run takes its references from Jena. The driver is `run.sh`.

**The files a limit produces,** in `<out>/<limit>g/`:
- `triplelight.warm.json`: the driver's per-query runs, with `t0`/`t1` and the times. That's where the timings live.
- `triplelight.s1.tsv` and `triplelight.s1.json`: the cgroup samples and the per-query join (`s1-report.py`).
  - **On macOS there is no cgroup v2,** so the sampler records why (`s1-1g.tsv`) and the report says `no samples` per
    query (`s1-1g.json`), as designed.
  - **On Linux the server runs in `systemd-run --scope -p MemoryMax=<limit>G -p MemorySwapMax=0`** (`lib.sh
    start_system`). The sampler and the join were checked in a privileged Linux container in #183.
- `triplelight.s1-cache.json`: per query, over its timed runs, the block-cache hits, misses and hit rate, the bytes
  read from store files, the read calls and the evictions, split into index orders and dictionary (`s1-cache.py`).
  - In this run, 43 traced requests were found for 42 expected; the extra one is the readiness probe. The last 42 were
    matched to the driver's requests and checked against each query's traced text, so `joined` is true. Example:
    `s1-cache-1g.json`.

At this size the limits don't bind and the caches are warm, so the hit rates are 1.0. The numbers mean nothing here;
only the layout does.
