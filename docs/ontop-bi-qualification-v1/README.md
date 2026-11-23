# Ontop BSBM BI qualification on DuckDB

The full BSBM BI arm did not qualify. Ontop 5.5.0 with DuckDB JDBC 2.0.0.0 opened the repository, then the first request reached its 120-second deadline with no saved answer. The remaining 59 requests were unvisited. A separately registered, untimed diagnostic used the first frozen instance of each template. Four of the eight representatives passed the original Jena answer policy.

| Template | Answer outcome | Native SQL bytes | UNION tokens |
| --- | --- | ---: | ---: |
| Q1 | Timeout | 1,352,456 | 592 |
| Q2 | Exact, 10 rows | 1,223 | 0 |
| Q3 | SQL serialization error | — | — |
| Q4 | Different binary32 values, inside the registered rounding envelope | 6,135 | 0 |
| Q5 | Answer mismatch, 0 rows versus 14 | — | — |
| Q6 | Lexical differences at unchanged binary32 values | 6,275 | 6 |
| Q7 | Exact, both empty | 1,557 | 0 |
| Q8 | SQL serialization error | — | — |

The SQL metadata came from separate reformulation-only workers. Q1 produced more than 1.35 MB of SQL in that call. The answer worker’s time was not partitioned by phase. The Q3, Q5 and Q8 probes returned errors before supplying SQL metadata. The Q3 and Q8 answer records preserve the exception chain through `SQLSerializationException`, which reported that it expected only DBConstants or NULLs in the subtree to be translated into SQL.

## Mapping and Q5 check

The sealed view has 10,102,032 rows. All 16 disjoint subject-hash partitions matched the source bag in both directions. The census counted 639 branches, including 593 for `rdf:type`, across 40 predicates and 14 object domains. The subject gate found zero blank or non-IRI subjects.

The offline Q5 check found the 30 `ProductType487` products in their grounded branch. All four join predicates had IRI objects. The single price branch covered 570,000 literals of datatype `bsbm:USD` without language tags. Frozen numeric anchors cover all 30 products and all eight distinct products in Jena’s 14 answer rows. Q4 uses the same `avg(xsd:float(xsd:string(?price)))` expression and passes the registered policy. These checks support the mapping’s reported coverage. The cause of the Q5 mismatch remains unresolved. The average subquery’s unbound projection of `?country` is one possible area to investigate.

The first view preparation stopped at its scratch guard. The partitioned retry verified the view and reached the original 512-branch cap. A separate census and mapping run then verified all 639 branches. The packet preserves both stops, the original suite timeout and the eight-template diagnostic. The diagnostic does not establish outcomes for the other instances of a template. No performance measurements were collected.

Run `python3 -IB replay.py packet.tar.gz` to verify the packet and reproduce `AUDIT.json`, including the mapping, typed answer checks, and Q5 mapping audit.
