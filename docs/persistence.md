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

The builder currently parses the entire input into an in-memory snapshot before writing the dictionary and six fixed-width index files. The dictionary is retained in memory while serving; index reads use bounded decoded-block caches. It does not provide compressed blocks, external sorting, updates, a WAL, or atomic replacement of a prior snapshot. A failed build may leave an incomplete new directory. The manifest is written last. Open verifies the full dictionary checksum plus each index header and block directory, and checks index lengths and counts; index blocks are checksummed and checked for order on cache misses. `rdfdb verify` explicitly re-reads all blocks, checks dictionary references, and validates exact equality among permutations. That exhaustive verification can be expensive and is not performed by `open()`. Snapshot files must stay immutable after open. Warm caches can still supply blocks after an external modification, while a failing disk read is returned as an error. Treat the directory as trusted local input: the checksums are not authenticated.

Open reads the full dictionary and block directories, so its time and memory grow with dictionary size. Neither a restarted process nor an empty application block cache guarantees a cold operating-system page cache. No large-scale or performance results have yet been established for this backend.

`write_disk_snapshot` accepts any snapshot and propagates scan errors, but currently materializes all triples and six indexes in memory during construction. Compaction therefore requires memory proportional to the dataset; it is not an external-sort implementation.

For durable updates on a disk base, use `rdfdb serve-store <directory> [port] --disk [--init data.nt]`. Updates are replayed from the WAL on restart; compaction writes another disk generation. Initial loading and compaction currently construct an in-memory snapshot.
