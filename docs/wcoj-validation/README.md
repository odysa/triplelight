# Integrated trie-order correctness check

All 14 LUBM-10 and all 20 WatDiv SF1 saved queries pass the three-way check:
index loops without statistics, legacy WCOJ without statistics, and WCOJ using
the new cost-based variable order. Complete RDF-term result multisets match
between all three, and each count matches the previously recorded Jena count.
Cyclic eligible BGPs take the trie path; acyclic BGPs use their normal fallback.

Run the q-error example with `--verify-wcoj` using the cohort data, query directory
and reference TSV. `*-verified.txt` lists every successful query; TSVs preserve
the estimates and errors. Source/binary provenance is recorded here; dataset
and query hashes are in the cohort provenance files. This validates correctness
on these fixtures, not general estimator accuracy or runtime performance.

The independent SF100 performance run found a large statistics-enabled overhead.
That issue remains open; statistics stay explicitly optional while it is profiled.
