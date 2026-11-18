# Prospective 100M numeric-input freeze

Freeze SHA-256: **63e127b60ac8de510a172748108639f0834e17bc3772e1d16ce226404aa58cca**.

Before reading any BSBM 100M BI result outputs, the 28 input-diagnostic cases and 32 numeric query instances were bound to independently checked anchors. All 8,304 output groups in those diagnostics agree with independent Jena counts. SQL input collection ran against the verified plain-triples DuckDB graph; Jena queried the authorized shared TDB2 graph in place, separately under the same machine-lock protocol. Both graph counts are 98,861,444, and the source NT SHA-256 is `6e4ef939f05b15677802d17257096de9da32a452fe33dd40c07d0b0537ee430d`. Count agreement does not prove full graph equivalence.

This bundle keeps the existing exact-anchor binary32 SUM/AVG arithmetic and validation rules unchanged. A new `freeze_numeric100.py` records the new cohort and its actual observation history: warmup mix 0 and measured mixes 1–3 were all unseen when frozen. The old 10M freezer and every protected 10M validator source retain their original hashes. No past result was reclassified. This freeze covers numeric q4/q5/q6 anchors only; it does not qualify complete BI outputs, authorize other datatype relaxations or establish 100M performance.

Validation: the original 19 arithmetic/qualification tests and three new cohort/coverage tests passed. The frozen-input loader accepted all 32 registered instances. Wrong freeze/dataset/workload identities and a modified copied anchor were rejected. `report.json`, `tests.log` and `postchecks.json` retain the checks and source identities.

The query-only Jena server was stopped before its lock was released. Source NT size/mtime remained unchanged; the shared TDB2 store's data-file metadata remained unchanged, with changes limited to its two expected lock files. No shared lock files were removed. The runtime directory retains the collection scripts, exact collector source copies, per-case logs, responses, process/status records and metadata snapshots. Completed SQL cases were saved individually; no BI candidate or competitor result outputs were read by these collectors.

`inputs/freeze.json` is immutable and binds all input files and tested validator sources. `file-sha256.json` additionally binds this report and its runtime evidence. Next: collect independent Jena BI reference outputs, then qualify each engine using explicitly registered policies before any timing comparison.
