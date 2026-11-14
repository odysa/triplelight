# Persistent snapshot prototype

This is an explicit read-only snapshot, not a transactional or updateable database. It is separate from the default in-memory server.

```sh
# Writes a new directory; fails if the destination already exists.
rdfdb build-disk data.nt ./snapshot --parallel-load=4
# Exhaustively check checksums, references, and permutation equality.
rdfdb verify ./snapshot
# A later process can reopen it.
rdfdb serve ./snapshot 7878 --disk
# A one-shot query also reopens it.
rdfdb query ./snapshot 'SELECT ?s WHERE { ?s ?p ?o } LIMIT 5' --disk
```

N-Triples disk construction uses scratch-backed external sorting to write a dictionary and six compressed index files. The dictionary is retained in memory while serving; index reads use bounded decoded-block caches. Durable updates and compaction are provided by `serve-store --disk`. A failed build may leave an incomplete new directory. The manifest is written last. Open verifies the full dictionary checksum plus each index header and block directory, and checks index lengths and counts; index blocks are checksummed and checked for order on cache misses. `rdfdb verify` explicitly re-reads all blocks, checks dictionary references, and validates exact equality among permutations. That exhaustive verification can be expensive and is not performed by `open()`. Snapshot files must stay immutable after open. Warm caches can still supply blocks after an external modification, while a failing disk read is returned as an error. Treat the directory as trusted local input: the checksums are not authenticated.

Open reads the full dictionary and block directories, so its time and memory grow with dictionary size. Neither a restarted process nor an empty application block cache guarantees a cold operating-system page cache. No large-scale or performance results have yet been established for this backend.

`write_disk_snapshot` streams a snapshot through scratch-backed sorted runs for term assignment and all six indexes. N-Triples `build-disk` uses the same external builder; Turtle currently uses the older in-memory loader. The run buffer target is 128 MiB, while merge heads, input records, run-path metadata and index block-directory metadata consume additional memory. Store startup still loads the dictionary into memory. The build requires scratch disk space and may leave an unpublished incomplete output directory after an error.

For durable updates on a disk base, use `rdfdb serve-store <directory> [port] --disk [--init data.nt]`. Updates are replayed from the WAL on restart; compaction writes another disk generation. N-Triples initialization and compaction use the external builder; opening the resulting store still loads the dictionary into memory.

The decoded block-cache target defaults to one quarter of physical RAM on macOS (256 MiB fallback elsewhere) and can be overridden with `--cache-mb N` or `--cache-mb=N`. This is a per-snapshot target divided among six indexes and 16 shards each, not a shared process-wide cap; simultaneously retained generations can each have their own cache. A minimum of one block per shard and allocator/metadata overhead are outside the nominal budget.

New snapshots use RDFIDX03 compressed blocks: bit-packed columns with first-column deltas and per-block minima for the other columns. RDFIDX02 snapshots remain readable. SHA-256 covers each stored block and its directory; checksums are not authentication. Compression ratio and throughput remain workload-dependent.
