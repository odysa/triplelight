# Original RDFLib-SQLAlchemy with an explicit setuptools dependency

RDFLib-SQLAlchemy 0.5.4 opened and reopened its SQLite stores after adding setuptools 80.9.0, an undeclared dependency that supplies the `pkg_resources` module. The [earlier declared-dependency profile](../rdflib-sqlite-qualification-v1/README.md) stopped at import. The new profile used a fresh environment and database and kept the published adapter code and all other package versions unchanged.

| RDFLib normalization | Graph length at load and reopen | Direct Graph iteration at load and reopen |
| --- | ---: | --- |
| Installed default | 8 | Nine triples. `"01"^^xsd:int` came back as `"1"^^xsd:int`, and `"INF"^^xsd:double` as `"inf"^^xsd:double`. The separate `"1"^^xsd:integer` was also present. |
| Disabled | 9 | Nine triples matching the exact typed reference bag. |

The original plugin reproduces the length and iteration discrepancy seen with BrickSchema. Its pinned count implementation projects distinct subject, predicate and object within the literal partition. The SQLite literal table records datatype in a separate column. Applying that projection to the retained typed triples yields eight for the default profile and nine with normalization disabled. This is a source-based explanation. The executed SQL was not traced. [Pinned original count implementation](https://github.com/RDFLib/rdflib-sqlalchemy/blob/2cf9689f9be750d52160df52713de215bdcde597/rdflib_sqlalchemy/sql.py)

Both profiles reopened the same five tables, reported DELETE journal mode and closed without error. The between-process inventories contained only the SQLite database. The retained database digests still match those inventories. The separate shared RDFLib `FILTER(false)` diagnostic returned nine rows in all four observations, against zero expected. It stays outside the storage verdict. No timing or SPARQL workload admission follows from this qualification.

The run used macOS ARM64, CPython 3.12.12, RDFLib 7.6.0, SQLAlchemy 1.4.54 and SQLite 3.50.4. The expected `pkg_resources` deprecation warning is retained. The BrickSchema profile was not rerun.

## Replay

Extract `packet.tar.gz` to a fresh directory and run the following commands from the extracted directory:

```bash
python3 -B replay.py . > audit.json
cmp audit.json AUDIT.json
```

`PACKET_FILES.json` gives the archived file digests. The packet includes the source, setup identity, raw observations and the schema and file inventory. The original database files remain in the run directory.
