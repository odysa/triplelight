# RDFLib SQLAlchemy with SQLite qualification

The BrickSchema SQLAlchemy adapter preserved all nine typed reference triples through a fresh-process reopen with RDFLib literal normalization disabled. With normalization enabled, direct iteration still returned nine triples but Graph length reported eight. The original RDFLib-SQLAlchemy release stopped at import in its declared-dependency environment, before creating a database.

| Profile | Graph length at load and reopen | Direct Graph iteration at load and reopen |
| --- | ---: | --- |
| Original RDFLib-SQLAlchemy 0.5.4 | Skipped | Import required `pkg_resources`, absent from the declared-dependency environment. |
| BrickSchema 0.7.1, default normalization | 8 | Nine triples. `"01"^^xsd:int` became `"1"^^xsd:int` and `"INF"^^xsd:double` became `"inf"^^xsd:double`. |
| BrickSchema 0.7.1, normalization disabled | 9 | Nine triples, matching the exact typed reference bag. |

In the default profile, iteration contains both `"1"^^xsd:int` and `"1"^^xsd:integer`. The inspected BrickSchema Graph length implementation issues counts over distinct subject, predicate and object values in each partition. Its literal partition stores datatype in a separate column, which that count projection omits. Applying the source projection to the retained triples yields the observed eight for the default profile and nine with normalization disabled. This is a source-based explanation of the length discrepancy. The run did not capture the executed SQL. The recorded SQLite schema and typed iteration show the datatype distinction is preserved in the observed rows. [Pinned count implementation](https://github.com/gtfierro/rdflib-sqlalchemy/blob/0b278dc0b2c8ce0044506a6bb18df6adff524f26/rdflib_sqlalchemy/sql.py)

Both BrickSchema profiles reopened the same tables. Each close returned without error. Both stores reported SQLite DELETE journal mode. The between-process inventories show the main database alone, with no WAL, rollback journal or shared-memory sidecar. A read-only byte inspection of the retained databases confirmed the literal table's DDL in files that still matched their pre-reopen digests. This was an ordinary close and reopen check.

The separate RDFLib evaluator diagnostic returned nine rows for `FILTER(false)` in all four BrickSchema observations, against zero expected. It ran through the shared evaluator and stays outside the storage verdict. Neither setup produced workload timing or workload admission.

The run used macOS ARM64, CPython 3.12.12, RDFLib 7.6.0 and SQLite 3.50.4 on the ready profile. The original plugin was paired with SQLAlchemy 1.4.54, and the BrickSchema fork ran with 2.0.48. Both environments used pinned wheels. Python 3.12 was chosen because SQLAlchemy 1.4.54 provides a matching wheel for it. Earlier RDFLib storage reports used Python 3.14.3.

The later [original-plugin qualification with explicit setuptools](../rdflib-sqlite-upstream-qualification-v1/README.md) completed a separate load and reopen check.

## Replay

Extract `packet.tar.gz` to a fresh directory and run from the extracted directory:

```bash
python3 -B replay.py . > audit.json
cmp audit.json AUDIT.json
```

`PACKET_FILES.json` lists all archived file digests. The setup failure traceback in the packet replaces the private harness path with a relative path and retains the temporary venv path. `setup/LOG_PROVENANCE.json` records both digests. The original traceback and database files remain in the run directory.
