# Persistent disk snapshots

```sh
# Creates a new snapshot directory; fails if it already exists.
triplelight build-disk data.nt ./snapshot --build-external
# Reopen for queries or check all checksums and cross-index consistency.
triplelight serve ./snapshot 7878 --disk
triplelight verify ./snapshot
# Serve durable updates backed by a disk snapshot and write-ahead log.
triplelight serve-store ./store 7878 --disk --init data.nt   # needs `--features server`
```

New snapshots have compressed RDFIDX03 triple blocks and two front-coded dictionary indexes, one sorted by term and one by ID. The dictionary and triple index directories are read at open; their blocks are read on demand through bounded decoded-block caches. Older RDFSNAP2 snapshots and RDFIDX02 indexes remain readable; the older dictionary format is loaded fully at open. A failed build can leave an incomplete new directory. The manifest is written last.

Open checks file versions, directory bounds and checksums. On a cache miss, a block's bytes are read using position-based I/O and validated before use; read failures propagate as storage errors. `triplelight verify` re-reads the blocks, checks dictionary agreement and triple references, and validates that all six permutations describe the same triples. This exhaustive check can be expensive and is not performed at open. Keep snapshot files immutable while open: a cached block can still be served after an external modification. Checksums detect accidental corruption; they do not authenticate files.

N-Triples initialization compares three times the input file length with `--build-memory-mb` (default: half physical RAM on macOS; 256 MiB fallback). It chooses an in-memory loader if the estimate fits and external sorting otherwise. The threshold is a strategy heuristic, not a hard peak-RSS limit. `--build-external` forces external sorting. Compaction uses external sorting. Turtle currently uses the in-memory loader. Scratch disk space is required; run metadata and block directories grow with input size. Compaction may retain old and new snapshots simultaneously while readers exist.

The decoded block-cache target defaults to one quarter of physical RAM on macOS (256 MiB fallback) and can be overridden with `--cache-mb N` or `--cache-mb=N`. It is divided among six triple indexes and two dictionary indexes per snapshot; dictionary index shares are capped at 384 MiB each. Cache allocations are on demand. This is a per-snapshot target, not a process-wide cap across retained generations. A minimum of one triple block per shard and allocator/metadata overhead are outside the nominal target.

A fresh process or an empty application cache does not imply a cold operating-system page cache.
