# Aggregate72 Virtuoso qualification passes

Virtuoso passes all 72 curated queries on both the primary exact RDF-term and preregistered COUNT-compatible bag gates: 30 LUBM-100 and 42 WatDiv100 requests. The complete raw response bags and hashes were independently replayed against the fixed Jena references. This is one correctness run per dataset, without a two-order timing result.

Fresh owned stores were loaded from the registered read-only N-Triples inputs. Before/after source hashes match; loader status rows finish at state 2 with NULL errors. Full named-graph counts match Jena: 13,409,395 LUBM triples and 10,914,314 WatDiv triples. HTTP requests explicitly supply the appropriate default-graph-uri while preserving the frozen query text.

The pinned image is openlink/virtuoso-opensource-7@sha256:0dbe1ab4fa0cb7bbafc1f6c0c2b0a5d6f22d918dbd17672f2ddb24580aa6756a, arm64. Containers use four CPUs and 12 GiB total memory/swap allowance with no extra swap. NumberOfBuffers is 1,360,000; MaxDirtyBuffers 1,000,000; ThreadsPerQuery 4; SPARQL maximum result rows 1,000,000,000; query execution cutoff 600 seconds. Selected non-secret INI fields were checked after execution. The client cutoff is 610 seconds. Each dataset loads and queries serially before its owned container is stopped and removed.

Recorded processes have exited, both owned containers are removed, and the exact benchmark lock is released. Resource guard and container state receipts are retained. Sampling is not a hard instantaneous storage quota. No OOM, timeout, missing query or failed query result was recorded. This preparation establishes neither comparative load performance nor matched query performance.

Credentials came from an existing private local file and were supplied through the environment/container-local shell; saved commands contain variable names only. Credential-related startup diagnostics are redacted, while query responses are complete. Database directories, private credentials and generated keys/certificates are excluded. The retained postcheck records this publication boundary. Do not copy database directories into reports.

The corpus is curated and previously inspected, not a blind holdout. Frozen queries, comparison policies and source identities are in ../aggregate72-preparation-v1; references in ../aggregate72-reference-v1. The next stage requires a separately registered matched timing protocol. SHA256SUMS.json hashes every report/evidence file except itself. Historical absolute paths require explicit adaptation before replay.
