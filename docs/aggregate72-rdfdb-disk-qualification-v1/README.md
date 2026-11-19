# Aggregate72 rdfdb disk qualification passes

The disk backend at `9a2f49b652a73466873cacd0a9205098e27d15f6` passes all 72 primary exact-term and COUNT-compatible bag gates against the fixed independent Jena responses: 30 LUBM-100 and 42 WatDiv100 requests. Complete raw response hashes and result bags were independently replayed. Full graph counts match Jena: 13,409,395 and 10,914,314 triples.

Two owned disk stores were freshly built from the registered read-only N-Triples inputs. Builds use four ingestion/Rayon workers and an explicit 8192 MiB build-memory path-selection setting; this setting is advisory, not a hard RSS limit. The server opens each store serially with --disk and --cache-mb=10240, waits for optimizer statistics, then runs the fixed queries. This native preparation has no hard process CPU or memory quota.

Both source file digests match before and after execution. Complete store file sizes and digests are identical before and after querying. Builds exit successfully, recorded processes are absent and the exact owned lock was released. Raw commands, build/server logs, source/store identities, responses, policy results and scratch guard receipts are retained. Store blobs are excluded from Git.

This supplies disk-backend correctness qualification; it is not a matched timing or comparative load result. Startup/build/query observations in logs remain preparation evidence. The curated cohort is not blind. Frozen workload/policies/input identities are in ../aggregate72-preparation-v1, references in ../aggregate72-reference-v1, and the tested source/binary identities in ../exact-decimal-qualification-v3. A fresh Linux build and separately frozen timing protocol are required before the planned same-VM four-engine comparison.

SHA256SUMS.json hashes every report/evidence file except itself. Historical absolute paths identify the actual run and need explicit adaptation before replay.
