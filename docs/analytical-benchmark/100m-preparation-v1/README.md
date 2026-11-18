# 100M BI input preparation

The shared N-Triples file is pinned at 25,965,260,343 bytes and SHA-256 `6e4ef939f05b15677802d17257096de9da32a452fe33dd40c07d0b0537ee430d`. Its inode, length and modification timestamp stayed unchanged during the read-only hash. This does not yet certify graph cardinality, term support or pool-to-graph consistency; those are checked during import and independent diagnostics.

The official BSBM parameter exporter produced 60 query instances from the existing `td_data100` pool with seed 808080: one warmup mix and three measured mixes. All query hashes are verified, and the manifest retains resource-pool and tool-JAR hashes. No 100M BI query was executed or candidate output inspected in this preparation window. The exported inputs are in `bench/analytical/workloads/bsbm100/`.

This is not the numeric validator freeze. The 100M exact-input anchors and independent Jena group counts have not been collected; a separately versioned input/policy bundle must be verified before qualification. The old 10M freezes remain unchanged and cannot certify this new graph. Likewise, this bounded cohort is not the official BSBM 50/500-mix throughput procedure.

The window used the shared-host lock and released it after hash/export verification. No larger import has started. The approved 48 GiB total owned-scratch cap and 40 GB host free-space floor apply to the next evaluation stage.
