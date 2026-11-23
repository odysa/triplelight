# Ontop aggregate answer qualification on DuckDB

Ontop 5.5.0 with DuckDB JDBC 2.0.0.0 matched the frozen Jena references exactly for all 30 registered LUBM-100 aggregate labels and all 41 registered WatDiv-100 aggregate labels. The checker compared typed result bags and separately checked the COUNT-compatible references. All 71 checks passed under each comparison. The WatDiv query `distinct_L2.rq` remains excluded because its mapped domain had not qualified.

| Dataset | Source rows | Grounded mapping branches | Non-IRI or blank subjects | Exact typed matches |
| --- | ---: | ---: | ---: | ---: |
| LUBM-100 | 13,409,395 | 32 | 0 | 30/30 |
| WatDiv-100 | 10,914,314 | 124 | 0 | 41/41 |

The first mapping used a column-valued predicate. Its flattened views matched the source bags in both directions on both datasets, but Ontop stopped during initialization with `NoGroundPredicateOntopInternalBugException` before executing any workload query. The grounded mapping uses a constant predicate in each branch and a constant IRI for each observed `rdf:type` class. The branch counts come from the separate read-only catalog checks.

The first grounded WatDiv run saved four exact answers before its DEBUG stdout exceeded the registered 2 MiB log ceiling during `watdiv/all_F2.rq`. One query was interrupted and 36 were unvisited. The separately registered retry kept the same ceiling and used WARN logging. It completed all 41 labels. The packet retains the first run and both initialization failures.

These are untimed aggregate qualification runs. BI and SQLite fall outside this packet’s scope.

Run `python3 -IB replay.py packet.tar.gz` from this directory to verify the archive manifest and regenerate the typed answer checks in `AUDIT.json`. The packet contains the frozen queries and references, mapping catalogs and generated mappings, answers and run receipts. The replay operates on the archived evidence without opening the source databases.
