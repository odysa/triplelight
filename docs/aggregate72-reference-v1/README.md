# Independent Jena references for 72 curated aggregates

All 72 queries now have complete Jena 6.2.0 reference responses: 30 on LUBM-100 and 42 on WatDiv100. Each reference validates against the separately registered projection and exact-integer COUNT policy; all group keys, binding states and row multiplicities remain intact. Self-comparison validates reference format and policy eligibility; it is not independent confirmation that Jena's answers are correct.

The successful attempt is evidence/aggregate72-reference-v2. Its reference-binding.json binds each raw response, its row count, the original rules freeze, and both dataset identities. Rules freeze SHA-256 remains `5cf92a76e599d1e8a1db1f56a3f5215c3b05fea989665754c02ce3f76bb7f365`; reference binding SHA-256 is `bb4e8d802c3ed4ecbf6bb917e469ff89e48a95606056084c3f506307f035d372`. It was created after reference collection, before new competitor responses. This is the previously disclosed curated cohort, not a blind holdout.

Each dataset was loaded from the hash-bound, read-only N-Triples file into its own native in-memory Fuseki process, with an 8 GiB Java heap and four active processors. Dataset COUNT responses and before/after input hashes are retained. Constant ASK readiness precedes exactly one awaited dataset count and then serial aggregate queries. Server query timeout is 600 seconds, client timeout 610 seconds. Both inputs match their registered hashes before and after execution. The server JAR hash, commands, source drivers, status, raw responses and logs are retained. All recorded owned processes were absent at the postcheck; the launcher released its exact lock after the supervised process ended.

## Failed preparation attempt retained

Attempt v1 had a harness error: a full dataset COUNT readiness probe timed out at the client after two seconds and retried, leaving overlapping requests on Jena. This was detected during the ten-minute audit. The owned supervisor was stopped; its process-group cleanup stopped the owned runner and server. Its launcher released the lock. No aggregate responses were accepted from v1. Its scripts, logs, abort explanation and exit receipts remain in evidence/aggregate72-reference-v1. The corrected v2 was a new attempt, without overwriting v1.

The successful attempt's sampled maximum allocated scratch usage was 16.053 GiB and minimum free disk space 227.741 GiB (see the exact resource summary below). Guards abort conservatively at 40 GiB allocated or 48 GiB free within the approved 48 GiB total scratch allowance. Sampling is not an instantaneous hard storage quota. These are preparation resource observations, not comparative engine memory measurements.

No competitor correctness result, engine eligibility or comparative timing is established here. The next steps are independent execution against these fixed references and a separately registered matched timing protocol. Timings in server logs are not benchmark results.

SHA256SUMS.json hashes every report and evidence file except itself. Historical absolute runtime paths in drivers are provenance; explicitly adapt them before replay. Database blobs and generated Fuseki run directories are omitted.
