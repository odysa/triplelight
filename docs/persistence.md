# Persistent snapshot prototype

This is an explicit read-only snapshot, not a transactional or updateable database. It is separate from the default in-memory server.

```sh
# Writes a new directory; fails if the destination already exists.
rdfdb build-disk data.nt ./snapshot --parallel-load=4
# A later process can reopen it.
rdfdb serve ./snapshot 7878 --disk
# A one-shot query also reopens it.
rdfdb query ./snapshot 'SELECT ?s WHERE { ?s ?p ?o } LIMIT 5' --disk
```

The builder currently parses the entire input into an in-memory snapshot before writing the dictionary and six fixed-width index files. The dictionary is retained in memory while serving; index reads use bounded decoded-block caches. It does not provide compressed blocks, external sorting, updates, a WAL, or atomic replacement of a prior snapshot. A failed build may leave an incomplete new directory. The manifest is written last, and open checks file hashes and validates ordering, entry counts and dictionary references. Exact equality among all permutations requires the explicit `verify_permutations()` method; `open()` does not perform it. Snapshot files must stay immutable after open. Warm caches can still supply blocks after an external modification, while a failing disk read is returned as an error. Treat the directory as trusted local input: the checksums are not authenticated.

Open reads every index for checksum and validation, affecting page cache. A timed query after open is not an operating-system cold-cache measurement. No large-scale or performance results have yet been established for this backend.
