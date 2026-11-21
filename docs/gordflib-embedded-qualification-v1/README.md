# goRDFlib v0.10.5 embedded Go fixture

goRDFlib passed all **40 typed SPARQL checks** across five routes on macOS ARM64 with Go 1.26.4. The native N-Triples parser emitted all nine exact fixture triples. Each query route reported a native length of nine and returned the same nine triples through native iteration.

| Boundary | Native length | Exact native triples | Typed checks |
|---|---:|---:|---:|
| Memory | 9 | 9 | 8/8 |
| Badger v4.9.1 load | 9 | 9 | 8/8 |
| Badger fresh read-only reopen | 9 | 9 | 8/8 |
| SQLite via modernc.org/sqlite v1.46.1 load | 9 | 9 | 8/8 |
| SQLite fresh normal reopen | 9 | 9 | 8/8 |

The Go query API supplied native term kinds, lexical forms, datatypes, language tags, projection metadata and the ASK result. The eight checks cover grouping, plain projection, DISTINCT, both integer counts, a zero-row projection, an unbound variable and false ASK. The same text `urn:iri` was preserved as both a distinct IRI and literal. The profiles share the goRDFlib query evaluator.

Both disk loaders closed successfully before a separate process opened their stores. Badger's five closed-store files matched byte for byte before and after read-only reopen. SQLite's 40 KiB database file matched after both closes. No SQLite WAL or SHM sidecar was present in either closed-store inventory. The SQLite worker used the normal constructor, which may write, so this is a clean-reopen check. Neither route exercised crash recovery.

Setup verified the goRDFlib module and backend versions and built the worker with module network lookups disabled. An earlier setup attempt stopped at offline module graph resolution before building the worker. The completed attempt explicitly prefetched the resolved module list before the offline phase. The packet includes both setup records, the frozen source, dependency receipts and BSD-3-Clause licence. [Pinned goRDFlib source](https://github.com/tggo/goRDFlib/tree/5f2bddd2e5f007b7aa053c22090356b0a616780f)

The nine-triple fixture checks correctness at this size. No workload timing was collected.

## Replay

Verify `packet.tar.gz` against `PACKET_FILES.json` and extract into a fresh directory. Run `python3 -I -B replay.py` from there. It verifies source and runtime receipts, checks the native and typed result records, compares the preserved store files, and reproduces `AUDIT.json` byte for byte.
