# Profile v4

TripleLight against QLever and Virtuoso on six suites, run from one command through the benchmark plan runner
(`bench/eval/plan.py`, task #130). It is profile v3's protocol (warm, 3 timed runs per query, 120 s timeout, QLever's
result cache cleared per request), now scored: every session is bound to a manifest and a machine-check receipt, and
a wrong answer, timeout or failed reset makes a session unscored instead of slow.

```sh
TOOLS=<dir with the datasets and stores> bench/profile/v4/run.sh <empty output dir>
```

`run.sh` writes one plan per suite for this machine (`make-plans.py`; plans hold absolute paths, so they are generated,
never committed) and runs them in turn. Each plan directory gets `report.md` and `report.json`.

| Suite | Dataset | Queries | Reference answers |
|---|---|---|---|
| bi100 | BSBM-100M | 60 BSBM BI (`queries/bi100`) | Jena 6.2.0, frozen |
| bsbm100 | BSBM-100M | 20 BSBM Explore (`bench/bsbm100q`) | Jena TDB2 |
| lubm1000 | LUBM-1000 | 14 LUBM (`bench/lubm`) | Jena TDB2 |
| agg-lubm1000 | LUBM-1000 | 30 aggregates (`queries/agg-lubm`) | QLever-derived (not Jena), as in v3 |
| watdiv100 | WatDiv-100M | 20 WatDiv (`bench/watdiv`) | Jena TDB2 |
| agg-watdiv100 | WatDiv-100M | 42 aggregates (`queries/agg-watdiv`) | Jena 6.2.0, frozen |

The references are profile v3's, with local paths removed from their provenance field; the answer digests are
unchanged. `configs/` records each engine's configuration as the manifest hashes it.

`run.sh` fixes the session budget to `--mem 12 --cpus 8` for every engine (refusing a different caller `BUDGET`), and the
config files the manifests hash say the same.

**Before the scored acceptance run** three pieces must be on main: the machine check (#296), the report (#305) and the
frozen BSBM BI case policy (#130). Until the policy is in, BI answers inside the registered rounding, lexical and
cutoff rules show as mismatches, and those sessions are unscored.

Each system gets 3 server sessions (`--sessions`), so queries under 5 ms can pass the noise gate in
`bench/eval/abba.py` when two profiles are compared.
