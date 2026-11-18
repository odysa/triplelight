# Aggregate72 QLever: COUNT-compatible qualification passes

QLever passes all 72 curated queries under the separately preregistered COUNT-compatible gate, while 0 of 72 pass the primary exact-term bag gate. Computed COUNT outputs use an integer datatype different from Jena's xsd:integer. The frozen compatibility rule normalizes only declared computed COUNT columns using validated exact nonnegative integer values; group keys, binding states and complete bag multiplicities remain unchanged. This is not an exact-term pass or a post-hoc policy change.

An independent replay verifies all raw response/reference hashes and 72 complete result bags. Every request has a retained cache-clear acknowledgement with five zero counters, checked before the request. Full graph counts match Jena: LUBM-100 13,409,395; WatDiv100 10,914,314. Index digests remain identical before and after serving. Recorded owned processes have exited, owned containers are removed and the exact benchmark lock is released.

## Pinned preparation

Two owned indexes were freshly built from the registered read-only N-Triples inputs. Both source hashes match before and after build; metadata confirms all six permutations, normal graph counts and QLever revision 18c097c. QLever image: adfreiburg/qlever@sha256:d7ecda47b4d0132c190b4a73279761fd1c614f24cba4e894ca1ba8117cef6727; local arm64 image ID e5555bafa0d0ba40260309fd7773948408ec68aea6fff8133cb169b98b0136ec. The VM reports 16,808,177,664 bytes of memory.

Builds run serially with four CPUs/threads, a 12 GiB container memory-and-swap total and 4 GB sort setting. Serving uses separate serial containers, read-only indexes, four CPUs, 12 GiB memory-and-swap total, 11 GB query setting, 1 MB cache caps, 600-second query limit and four maximum query threads. Live settings and commands are retained. No query output was discarded and there were no recorded query failures or timeouts.

The supervisor samples the entire approved scratch root and enforces conservative abort thresholds; sampling is not an instantaneous storage quota. Build and query elapsed observations in logs are preparation data, not comparative timing or memory benchmarks. Index blobs are excluded from Git; manifests, metadata, input identities, commands, status and guard receipts remain.

The same frozen rules apply to the scoped DuckDB baseline and rdfdb native-memory qualification, each of which separately passes all 72 primary and COUNT-compatible comparisons. There is no matched aggregate timing result yet, and Virtuoso qualification remains pending. The workload is curated from earlier count-pushdown work and inspected fixtures, not blind. Sources/policy are in ../aggregate72-preparation-v1 and independent references in ../aggregate72-reference-v1.

SHA256SUMS.json hashes every report/evidence file except itself. Historical absolute paths identify the actual runs and require explicit adaptation before replay.
