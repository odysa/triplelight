# Embedded Blazegraph 2.1.5 LUBM-100 qualification

Blazegraph 2.1.5 passed **30 of 30 exact typed answer checks** on the registered LUBM-100 aggregate workload. The separate COUNT-compatible gate also passed all 30. The embedded store reported **13,409,395 total triples and 13,409,395 explicit triples**, both after load and after a fresh-process reopen. This run establishes correctness for one profile and collects no scored query latency.

## Profile

The pinned Maven jar ran in process on Linux ARM64 and Temurin 21.0.13+9. It used triples-only DiskRW, no XSD inlining, NoAxioms, truth maintenance disabled and native tuple queries with `includeInferred=true`. The profile exposes all stored statements, which the exact counts show were all explicit. The earlier [COUNT fixture](../blazegraph-embedded-count-fixture-v1/README.md) retains the separate explicit-only fast range-count assertion. The revised worker passed exact typed SELECT, integer COUNT and FILTER(false) on its own nine-triple streaming fixture before registration.

The input was the existing 2,377,133,547-byte N-Triples file. SHA-256 matched before and after import. Two previous Jena TDB2 passes and a prior PyOxigraph load corroborate the unique-triple count for the same hashed input. Import streamed through Rio with datatype normalization off, with a 10,000-statement buffer, a zero-length queue and one commit after parse. The three workload stages ran in separate JVMs. The reopen and query stages verified the journal's SHA-256 before opening it.

## Recorded outcomes

| Boundary | Gate | Observation |
|---|---|---|
| Load | Native total and explicit count | 13,409,395 each |
| Fresh-process reopen | Native total and explicit count | 13,409,395 each |
| Query | Jena exact typed result bags and native projection | 30/30 labels, 60,203 rows across the labels |
| Query | Separate exact integer COUNT compatibility | 30/30 labels |
| Journal after load and reopen | Size and identity | 1,964,179,456 apparent bytes, 1,731,067,904 allocated bytes, identical SHA-256 |

The 30 registered labels contain 26 distinct query texts. They are curated aggregate rewrites, including two grouped results. The primary gate preserves term kind, lexical form, datatype, language and multiplicity under the frozen RDF-term policy. The separate compatibility gate applies only to computed COUNT columns. The full input and query manifest retains the reference bindings.

Resource limits were four CPUs, 12 GiB container memory including swap and an 8 GiB JVM heap. The sampled root allocation peaked at 1,753,456,640 bytes, and sampled host free space stayed at or above 83,447,226,368 bytes. The largest observed cgroup memory peak was 2,591,268,864 bytes during load. These are resource receipts from the qualification, with no performance comparison. All three workload containers exited successfully and were removed before the machine lock was released.

## Replay

Register SHA-256 `b1fcb44f4da6b155f819ba59e8aa76d91f97c4996a33a8f499a4931d0f54b290` binds the exact input and query manifest, the compiled runtime and the passing fixture. The packet contains its launch binding, the frozen source and query bundle, Jena answers, native result records, two prior count sources, the stage receipts and the engine-free audit. The dataset, database journal and Maven jar are identified by hashes and omitted.

Verify the archive and its members against `PACKET_FILES.json`, extract into a fresh directory and run `python3 -I -B replay.py` from there. It recomputes the typed answer results and verifies the source, registration, count, journal-handoff, resource and cleanup receipts. Standard output matches `AUDIT.json`. The replay reads saved observations and requires no JVM.
