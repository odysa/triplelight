# Analytical RDF benchmark — task25

Status: **in development**. Five offline qualification tests pass. Java workload
export, DuckDB import/SQL, actual database fixtures and large-data comparisons
have not yet run. There are no BI performance numbers in this directory.

The goal is an embedded analytical RDF engine. The earlier 20 BSBM exploration
queries are a different workload. BSBM's author describes BI as GROUP BY/COUNT
analytics touching large portions of the dataset in this
[primary announcement](https://lists.w3.org/Archives/Public/public-lod/2013Apr/0409.html).
The eight BI templates and their descriptions, qualification columns and
weighted mix are preserved unmodified under `bsbm-bi/templates/`. `upstream.json`
records byte hashes and a pinned upstream mirror comparison.

## Workload generation

`ExportBiQueries.java` calls BSBM's own `Query`, `QueryMix` and
`LocalSPARQLParameterPool`. `make_workload.py` builds/runs that exporter without
an endpoint, recovers the exact parameter replacements and records input,
template, resource and jar hashes. Repeated placeholders must agree. It uses
the original consecutive-date generator, not a substitute calendar calculation.

The default seed is 808080. A mix has 15 query instances in this order:
`1,3,2,2,4,5,5,6,5,6,6,7,8,7,6`. A planned initial cohort has one warmup mix and
three measured mixes. This differs from the original tool's defaults of 50
warmup and 500 measured mixes; report it as a bounded BI cohort, not an official
full BSBM throughput score. Weighted mix totals require every scheduled query
to complete and qualify. Errors, unsupported operators and timeouts are not
zero-cost successes and are not removed from a headline score.

## DuckDB baseline

`duck_load.py` streams N-Triples through Arrow batches into one table:

```text
triples(s VARCHAR, p VARCHAR,
        o STRUCT(kind UTINYINT, value VARCHAR, datatype VARCHAR, lang VARCHAR))
```

RDF graph duplicates collapse at import. Literal lexical form, datatype and
language are retained; plain/xsd:string and language-case equivalences are
normalized. This corpus-scoped baseline requires named resources and rejects
blank nodes instead of conflating them with IRIs. It has no property tables,
materialized joins, aggregate cache or numeric index. DuckDB chooses its physical
compression and plans. Input SHA-256 and raw-versus-unique triple counts must
match the pinned dataset before the store is usable as evidence.

`bi_sql.py` contains hand translations for these eight templates only; it is not
a SPARQL-to-SQL frontend. Resource joins check RDF kind. Prices use lexical casts
even with the custom BSBM USD datatype. The q5 price subquery has unbound
`?country`, so its join is compatible with each bound outer country; SQL NULL
equality would be incorrect. q6 keeps bag UNION ALL and distinguishes the global
average's scope from the outer query's reviewer requirement. Invalid numeric
aggregate inputs must not silently become SQL AVG's ignored NULL values.

## Qualification contract

The supplied BI `*valid.txt` files omit several computed averages/ratios. Our
oracle checks all output columns, integer counts, RDF datatypes, multiplicities
and ordering. Sourced RDF terms keep exact identity. Computed xsd:float outputs
have a preregistered maximum difference of four binary32 ULPs; datatype changes
are rejected. Exact term-bag equality is reported separately, and every numeric
deviation is retained. A discrepancy is investigated rather than widening the
tolerance after seeing results.

q7 needs a different oracle: its inner top 1,000 offer-count ranking has no
tie-breaker and is filtered afterward. An untimed diagnostic collects every
product's count and blocked-country flag. Products strictly above the cutoff
must survive if eligible; only an allowed subset of boundary-tied products may
survive. The number of survivors is bounded by the available boundary slots and
blocked products. We do not append a tie-breaker to the official query or report
permitted alternatives as wrong bags.

The planned actual-engine fixtures cover q5 unbound compatibility, q6 bag and
scope behavior, invalid/unbound numeric aggregates, float promotion, empty
groups, ordering and q7 boundary ties. The current standard-library tests exercise
only the oracle/parameter recovery; they are not engine qualification.

## Execution and resource boundaries

Use an exclusive `/tmp/rdf-bench.lock` window before compilation, imports, engine
fixtures or timings. Keep source versions, runtime binaries, query parameters,
statistics readiness, thread/cache settings and output interfaces pinned.
Preparation and full result consumption are separate measurements; deferred
result handles do not count as completed work. Hashing and qualification remain
outside the timed query path.

The competitor ranking is planned in a matched Linux VM environment for rdfdb,
QLever, Virtuoso and DuckDB. Host embedded API measurements are a separate track.
Record actual RSS, temporary bytes/spill and failures rather than inferring
larger-than-memory support from a configured cache or memory target. The 10M
pilot determines disk requirements before requesting any scratch allocation
above the shared-host limit for 100M.

Offline checks: `python -m unittest discover -s bench/analytical -v`.
