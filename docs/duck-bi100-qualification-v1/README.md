# Untimed plain-triples DuckDB qualification at BSBM 100M

All **60 requests** passed the primary registered checks, COUNT-compatible checks and value-compatible checks. **42 of 60** also had exactly equal RDF-term bags; the remaining differences are accepted numeric differences under the preregistered rules. A primary pass includes frozen numeric tolerances and must not be described as exact RDF-term equality or global SPARQL conformance. No comparative execution-time result is reported here.

DuckDB 2.0.0 queried the same plain lossless typed triples table built from NT SHA-256 `6e4ef939f05b15677802d17257096de9da32a452fe33dd40c07d0b0537ee430d`, with 98,861,444 unique triples. There are no property tables or extra numeric indexes. The database SHA-256 before and after qualification was `1ad0939f1aead75a8b21f157e349f5a8d1db527305d4d0424c5c014d87aa8f9d`; collector source identities also stayed unchanged. This was native SQL qualification on the host, with four threads, an 8 GB DuckDB memory setting and 8 GB spill allowance, not the later matched-VM timing protocol.

The independent reference bundle contains 59 original full-query Jena responses and one explicitly labelled group-key-partitioned Jena q1 response. The partition protocol was validated independently and did not use DuckDB or any engine-under-test output to define its expected result. See `../jena-bi100-reference-v1/` for all reference attempts, timeouts and proof checks.

The 100M exact-input freeze is `63e127b60ac8de510a172748108639f0834e17bc3772e1d16ce226404aa58cca`. The final value-policy binding is `7b3f640bbab516db80ceba87549b9670802a93c5ff77181bdda66c0855fc3dfb`; the COUNT-only policy is `5049fb75210fc6fc78ebf1fb2553b88baef197ae73495b70f110a203a70a4663`. Those rules were registered before these candidate outputs were collected. No bound or reference was changed in response to the results.

Postchecking replayed all 60 primary checks, all 60 COUNT/value checks, all translated SQL sources and all reference hashes. The existing runner also checked the full q7 diagnostic against Jena at execution time; its separate diagnostic result rows were not saved, so the retained postcheck replays q7 shape, allowed-cutoff membership and ordering against the preserved Jena diagnostic.

`run/queries/` retains each translated SQL query, result JSON and the original qualification matrix. `run/postchecks.json` records per-output/query/reference hashes, separate policy scores and exact-term-bag observations. `run/` also retains the command, source copies, database/source checks, runner status and independent sampled disk-guard receipts. Both owned processes exited before the exact owner lock was released. The memory/spill settings are configurations, not measured process-peak guarantees.

`file-sha256.json` binds the archived evidence. Next: qualify rdfdb using the freshly built, separately pinned 100M store, then qualify competitors before a same-VM performance comparison.
