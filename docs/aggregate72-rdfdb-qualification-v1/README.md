# Aggregate72 rdfdb native-memory qualification passes

rdfdb at `9a2f49b652a73466873cacd0a9205098e27d15f6` matches the independent Jena references on all 72 primary exact-term and COUNT-compatible comparisons: 30 LUBM-100 and 42 WatDiv100 queries. Complete result bags, including group keys, binding states and multiplicity, were independently replayed. No policy or workload changed.

Each dataset was loaded directly from its registered read-only N-Triples source into a fresh native memory server. Ingestion and Rayon were configured for four workers; HTTP worker threads use the CLI's available-core default, with requests issued serially. There is no hard native-process memory or CPU quota. The server waits for optimizer statistics before querying; full graph COUNT matches Jena: 13,409,395 for LUBM and 10,914,314 for WatDiv. Both input digests match their registered hashes before and after execution.

The HTTP client preserves complete response JSON and compares it against the fixed hash-bound references using the separate curated COUNT policy. A post-run replay confirms all 72 result bags and body hashes. Registered validator source hashes, rdfdb source files and executable digest are unchanged. The supervisor and both server processes exited before the wrapper released its exact owned lock; process and scratch-guard receipts are retained.

This is native-memory correctness qualification, not disk-backend qualification, a matched performance run, a comparative load measurement or general SPARQL conformance evidence. Build/startup/query observations in logs are preparation data only. Resource guards sample the approved scratch root and are not hard instantaneous quotas. The cohort is curated from earlier count-pushdown work and inspected fixtures, not blind.

Frozen queries, input identities, translation and policy sources are in ../aggregate72-preparation-v1; independent references in ../aggregate72-reference-v1. DuckDB's separate correctness result is in ../aggregate72-duck-qualification-v1. Other RDF-engine checks and a separately registered matched timing protocol remain pending.

SHA256SUMS.json hashes every report/evidence file except itself. Absolute runtime paths identify the actual run; adapt them explicitly before replay. The tested binary/source identity is preserved in ../exact-decimal-qualification-v3, with executables excluded from Git.
