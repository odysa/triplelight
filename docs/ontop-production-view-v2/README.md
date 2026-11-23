# Ontop qualification with a DuckDB view

Ontop 5.5.0 read a nine-row typed-triples fixture through DuckDB JDBC 2.0.0.0 on macOS ARM64 with Temurin 25.0.5.1+2-LTS. The registered query run used a non-materialized view over the original triples table. The preceding direct R2RML mapping attempt reached JDBC successfully and stopped during Ontop initialization because it could not resolve the nested field `o.kind`. No SPARQL query ran in that attempt.

```sql
CREATE VIEW ontop_triples AS
SELECT s, p, o.kind AS kind, o.value AS lexical,
       o.datatype AS datatype, o.lang AS lang
FROM triples
```

The source checks found equal row bags between the view and the explicit projection in both directions. Both paths detected the injected invalid row, and rolling back restored all nine valid rows. The view changes which columns Ontop sees and leaves the physical base table intact. [JDBC source checks](answer/jdbc-source.json) · [R2RML mapping](replay/tiny-production-fixture-v2/mapping.ttl)

| Check | Outcome |
| --- | --- |
| Plain typed answer bag | Exact match for all nine terms |
| Total count | 9, exact match |
| Grouped typed answer bag | Exact match |
| Resource-only distinct count | 3, exact match |
| Mixed-term distinct count (diagnostic) | 8, reference 9 |

The fixture passed the four required checks. The mixed-term distinct count remains incorrect for this fixture. Full workload answer qualification and performance measurements are pending. [Typed answers and reformulation digests](AUDIT.json) · [Run receipt](FINAL.json)

Replay the saved typed answers with Python 3 from this directory. It prints the recorded audit JSON and requires only the standard library.

```sh
python3 -B replay/check_production_fixture.py answer
```
