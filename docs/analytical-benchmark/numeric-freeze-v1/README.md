# Frozen numeric validator v1

Freeze SHA-256: `a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244`. The original uncompressed runtime bundle is
`/private/tmp/rdf-jianlin-analytical25/numeric-diagnostics-10m`.

The source and input files named by freeze.json are immutable. Large JSON files
are published losslessly as .json.gz; compressed_artifacts.json binds both raw
and compressed bytes. Restore them in a copy before loading FrozenInputs.
The copied validator_sources match every runtime source hash in the freeze.
Additional setup/test modules are separately hashed and do not change the rule.

Independent Jena cardinalities agree on all 31 parameter cases, covering 22,381
entity groups and all 32 AVG-derived query instances. Nineteen tests pass. The
failed mock-metadata unit run is retained separately; the fixture was corrected
before the final passing test/source report and freeze.

Mix 0 outputs had already been examined and stay exploratory under the original
failed gate. Mixes 1–3 candidate outputs had not been inspected at freeze.
The collector's old candidate_outputs_read=false field refers to the collector
not reading engine result files; it does not erase prior mix 0 exposure.
Earlier validator_frozen=false fields describe creation-stage snapshots;
freeze.json is the final authoritative state for this version.

The three-engine small-graph integration smoke passed before the final raw
reference-output retention fix. A dedicated final unit regression verifies
that failing reference outputs are preserved. The final source-bound suite has
19 passing tests. Upcoming holdout execution is still required.
