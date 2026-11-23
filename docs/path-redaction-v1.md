# Path redaction (2026-11-23)

The repository is public, so absolute paths into the authors' machines were removed from the evidence files below. In logs and JSON, an agent workspace
root became `<workspace>`, the home directory `<home>` and `/private/tmp` `<scratch>`; the two `run.sh` scripts now take their directories from
the environment variables `OUT_DIR`, `DATA_DIR` and (cache sweep only) `REPO_DIR` and fail with a usage message when unset. Nothing else changed.
`ci/check_paths.py` fails the `notices` CI job if a path of that kind comes back (a line may quote the pattern if it carries the marker `<redaction-note>`).

Digests are the first 12 hex digits of the file's SHA-256, before and after.

| File | Before | After |
|---|---|---|
| `bench/results/bsbm10/jena-tdb2.server.log` | 39b060fee64f | fbb25cb52f61 |
| `bench/results/lubm100/jena-tdb2.server.log` | a83bc6c0fa48 | 5f71123b59a7 |
| `bench/results/lubm100/virtuoso.load.log` | c787d34a2f57 | 995025b6d50a |
| `bench/results/r9-bsbm100/jena-tdb2.server.log` | 1dd010e91e66 | 7fa2e21a6119 |
| `bench/results/r9-lubm1000/jena-tdb2.server.log` | 9fd648d6580f | 0414c7919080 |
| `bench/results/round1-lubm100/jena-tdb2.server.log` | 7dc8874e7887 | 59bf40c8d388 |
| `bench/results/round1-watdiv100/jena-tdb2.server.log` | 727353ee9755 | 596a76833190 |
| `bench/results/round2-lubm100/jena-tdb2.server.log` | 82ebcb6afb16 | 9d78aec79b79 |
| `bench/results/round2-watdiv100/jena-tdb2.server.log` | ac5fa6fff6ea | dbee557fcf82 |
| `bench/results/round3-lubm100/jena-tdb2.server.log` | c8991ba3a916 | cf34c798d5b6 |
| `bench/results/round3-watdiv100/jena-tdb2.server.log` | efb2925bacd9 | 53ead8c33bd2 |
| `bench/results/round4-lubm100.1794711043/jena-tdb2.server.log` | 28dfc4af9d74 | 95f5930c80a5 |
| `bench/results/round4-watdiv100.1794711788/jena-tdb2.server.log` | 40a97e52ddd2 | 624aaa8dd520 |
| `bench/results/round6-vm-bsbm10/jena-tdb2.server.log` | 1805cd9583df | 61863dcc10bf |
| `bench/results/round6-vm-lubm100/jena-tdb2.server.log` | cec9fe3aa615 | e2d69385cffb |
| `bench/results/round6-vm-watdiv100/jena-tdb2.server.log` | 543fe76af85f | 2d319b7bbfcd |
| `bench/results/watdiv100/jena-tdb2.server.log` | 0bfbf5950409 | b63167104df7 |
| `bench/results/watdiv100/virtuoso.load.log` | 98bad08e5c5e | 4dabc2c8cb24 |
| `docs/build-compare-orders-v1/run.sh` | 0b7c65b6c045 | eefbaa8c66a2 |
| `docs/cache-budget-sweep-v1/run.sh` | 6edfdf77c699 | d3acb587eea8 |
| `docs/count-pushdown/alltargets-v2.log` | 82e14e2298bb | a9017406763f |
| `docs/count-pushdown/alltargets-v3.log` | 31914355aba3 | 34ba1ae9bea2 |
| `docs/count-pushdown/alltargets.log` | a968f8c4d6c0 | 224fd0d6ed1d |
| `docs/count-pushdown/integration-rebased.log` | 14457c6cd1fb | 9d27ba01820b |
| `docs/count-pushdown/provenance.json` | e7e5d0f602d4 | cd0ad3061b10 |
| `docs/count-pushdown/results/jena-lubm-checks.json` | dea83f2bb76f | 9a289b561ae5 |
| `docs/count-pushdown/results/jena-lubm.log` | a5702eee4b65 | 53aac9d194b2 |
| `docs/count-pushdown/results/jena-watdiv-checks.json` | 7c636ea1832d | 330f65d9bca5 |
| `docs/count-pushdown/results/jena-watdiv.log` | fd2a709ecd7a | fd602d334681 |
| `docs/ladybug-hybrid-explore-v1/RESULTS.json` | 3c50ff36edf6 | 16e319aef459 |
