# Curated aggregate SQL translator checks

The scoped translator accepts all 72 prepared flat-BGP COUNT queries. Seven small DuckDB 2.0.0 checks pass; seven separate Jena 6.2.0 fixture outputs match complete RDF-term bags exactly. These cases cover empty global versus explicit grouping, distinct numeric lexical forms, repeated variables, cross-position joins, bag multiplicity and grouped serialization.

The translator operates on the existing lexical `triples(s,p,o)` schema, using complete RDF-term structs for join/group/DISTINCT keys. It accepts only named-IRI constants, variables, flat triple patterns and COUNT projections with optional variable grouping. It rejects other syntax; it is not a general SPARQL frontend. The loader's graph deduplication and named-subject contract still apply.

The raw fixture inputs and both engines' outputs are retained here. `provenance.json` identifies the tested source. These tests do not qualify the 72 real-data queries and provide no timing comparison. No frozen BSBM BI validator or COUNT policy was edited.

To repeat the translator unit checks from the repository root, use the benchmark Python environment with DuckDB installed:

```sh
python -m unittest discover -s bench/analytical -p test_aggregate_sql.py -v
```

For each retained `.rq`, the independent reference command was Jena 6.2.0 `arq --data=data.nt --query=<case>.rq --results=JSON`. DuckDB loaded the five retained statements into the same typed lexical triples schema, executed `aggregate_sql.translate(query).sql`, and serialized through `Translation.result_json`. `qualify(0, actual, reference, max_ulps=0)` required exact complete term bags. All tests and oracle generation ran under the owned benchmark lock, without collecting performance scores.
