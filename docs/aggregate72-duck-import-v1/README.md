# Aggregate72 DuckDB stores: preparation only

Both serial imports completed and match the independently collected Jena graph counts: 13,409,395 triples for LUBM-100 and 10,914,314 for WatDiv100. The frozen source digests match the registered inputs. Each store is a lossless typed triples table, deduplicated for graph set semantics; there are no property tables or extra numeric indexes.

The existing frozen loader ran with DuckDB 2.0.0, PyArrow 26.0.1, four threads, an 8 GB working-memory setting and 8 GB spill cap. Exact commands, progress, exit codes, source and completed database digests, counts and resource receipts are retained. Database blobs are excluded from Git. Loader sources and query comparison rules are frozen in ../aggregate72-preparation-v1; Jena responses are in ../aggregate72-reference-v1.

postchecks.json verifies successful imports, registered source digests, complete graph counts and absence of recorded owned processes. The supervisor samples the entire approved scratch root and aborts conservatively at 40 GiB allocated or 48 GiB free. Sampling is not a hard instantaneous quota. Per-import RSS includes Python, Arrow and DuckDB; spill observations are sampled file lengths. This preparation is not a comparative load or memory benchmark. There is no aggregate query qualification or timing result yet.

SHA256SUMS.json hashes every report/evidence file except itself. Historical absolute paths in drivers identify the actual run; adapt them explicitly before replay.
