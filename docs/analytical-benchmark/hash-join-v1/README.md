# Compatible-mapping hash join: frozen BI correctness check

Candidate `738f0cb72ae9a343fd2d7941dbbbec0659203143` passes all 60 unchanged frozen BSBM BI checks. Complete RDF-term bags match pinned main `71ab35cc9e0e512e469b16ad23a56f46d9ee308e` on all 60 outputs, and all 36 outputs with outer ORDER BY match their full ordered sequences. q7 also passes the existing tie-aware cutoff oracle. This is a finite-workload correctness gate, not a proof of equivalence on arbitrary SPARQL.

This candidate is separate from the earlier `54bee3a` equality rewrite and is based on it. It hashes subquery result rows only on columns bound in every row on both sides. Matching buckets still undergo the original compatible-mapping merge, retaining left order then right order. With no guaranteed bound shared key, it retains the nested loop. Kaiming reported the full release suite passing before this independent workload qualification; this report does not claim a second full Rust-suite run.

The candidate was built from isolated pinned source with one Cargo job on the Mac, then served the existing owned 10M disk store with four Rayon threads and a 10,240 MiB cache target. The control outputs came from the earlier matched-VM run. This host/VM comparison supplies correctness evidence only: **elapsed values here are not an A/B speed result**. The frozen numeric bounds, datatype checks, identities, counts, membership and order rules were not changed.

Source and binary hashes, commands, all query responses, per-instance oracle checks and complete bag/sequence comparisons are retained. The binary post-hash matches, the store manifest identity is unchanged, all 60 query hashes and all 32 enveloped input bindings pass the final postcheck. The server and runner exited and the owned lock was released before the handoff.

`file-manifest.json` records raw/stored hashes, with lossless gzip for larger JSON/response files. Native binary and database files are excluded while their provenance is retained. This result neither replaces the older matched-VM timing table nor completes the pending 100M and curated aggregate comparisons.
