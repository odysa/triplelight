# Aggregate72 SQL baseline: correctness qualification passes

All 72 curated queries match the independently collected Jena references on both the primary exact RDF-term bag gate and the separately frozen COUNT-compatible gate: 30 LUBM-100 and 42 WatDiv100 requests. This qualifies the scoped typed-triples SQL translation for this cohort. It supplies no matched performance result and makes no claim about general SPARQL-to-SQL translation.

The same registered workload, SQL translation hashes, input identities and separate COUNT policy were used throughout. Complete raw JSON responses and per-query checks are retained. A subsequent independent comparison replayed every complete result bag against its hash-bound reference and verified output hashes. Completed database digests remain identical to their import digests. Sources and policies are in ../aggregate72-preparation-v1; references in ../aggregate72-reference-v1; import records in ../aggregate72-duck-import-v1.

Each query runs serially in a fresh DuckDB 2.0.0/PyArrow 26.0.1 Python process, with the store opened read-only, four threads, 8 GB working-memory setting, 8 GB spill cap and a 600-second process cutoff. These process and cache choices are for correctness preparation, not a timing protocol. Logs may contain elapsed observations; they are not comparative benchmark timings. There were no recorded query process failures or timeouts, and no results were discarded.

The supervised runner waited for all query children, exited successfully and released its owned lock. Process-exit and resource guard receipts are retained; sampling is not an instantaneous quota. Native rdfdb and other RDF-engine qualification on this cohort, followed by a separately registered matched timing protocol, remain pending.

This is a curated cohort from earlier count-pushdown work and inspected fixtures, not a blind holdout. SHA256SUMS.json hashes every report/evidence file except itself. Absolute paths in drivers are historical provenance and require explicit adaptation before replay.
