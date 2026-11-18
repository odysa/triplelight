# Analytical RDF benchmark — task25

Status: **in development**. Sixty BSBM-10M BI query instances and 72 curated
LUBM/WatDiv aggregate cases are prepared. The corrected engine passes normal,
actual cutoff, invalid numeric and empty/unbound fixtures against Jena; the
plain-triples DuckDB translation passes all requested BI fixture cases. See the
[qualification report](../../docs/analytical-benchmark/README.md).
All 45 confirmatory 10M instances pass under the frozen exact-anchor rule for
Jena, DuckDB, both main rdfdb grouping paths and a separate wider accumulator.
The matched-VM 10M pilot is now retained in the report: rdfdb and DuckDB qualify,
with rdfdb slower in both passes. Competitor output failures prevent a complete
four-engine BI ranking. The 100M and curated aggregate comparisons remain pending.

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
match the pinned dataset before the store is usable as evidence. Imports record
whole-process peak RSS and spill file lengths sampled every 200 ms. The spill
figure can miss shorter peaks and is not allocated disk blocks; configured memory
and spill limits are reported separately from these observations.

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

Actual-engine fixtures cover all eight templates, q5 unbound compatibility,
q6 bag/scope behavior, actual q7 cutoff ties, and invalid numeric inputs.
An empty-graph BI fixture and `aggregate_check.py` additionally check empty
groups and mixed bound/unbound aggregate inputs; their execution status must be
read from the qualification report. The latter is a Jena/rdfdb diagnostic, not
a DuckDB SQL translation or a timed BI query. The standard-library tests
exercise only the oracle/parameter recovery; they are not engine qualification.

## Real-parameter qualification

`workload_check.py` consumes the exported manifest and verifies each query hash.
Run one engine at a time: Jena references first, then DuckDB and rdfdb in separate
processes. It checks complete results against the reference with the same numeric,
ordering and q7-cutoff rules as the small fixture. `--limit` explicitly marks a
partial workload; it is a diagnostic, not a complete weighted-mix result. The
DuckDB mode requires the successful pinned import report. Query errors and
missing references are retained as failures. There is no timing score in this
runner. Source/runtime versions, process supervision and source-file postchecks
must accompany the saved matrices.

Current DuckDB query runs can configure a separate empty owned spill directory
and maximum spill size, in addition to the memory setting. Those are settings,
not evidence of observed peak use. The initial real-parameter cohort uses its
snapshotted earlier checker; consult that source for its exact configuration.

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

The curated aggregate translator is `aggregate_sql.py`, scoped to the 72 flat-BGP COUNT queries. Seven tiny-graph unit checks and seven independent Jena fixture comparisons pass; see `docs/analytical-benchmark/aggregate-sql-fixture-v1/`. This is not yet a real-data aggregate performance comparison.

The guarded equality rewrite at `54bee3a` separately passes all 60 frozen BI instances, with complete bags and all 36 ordered sequences matching pinned main. See `docs/analytical-benchmark/eq-unify-v1/`; the host/VM output comparison supplies no A/B speed claim.

The subsequent two-engine matched-VM rerun on main `738f0cb` is in `docs/analytical-benchmark/matched-vm-round2-v1/`. All 240 requests qualify; rdfdb takes 27.23 / 28.54 seconds against DuckDB’s 55.32 / 64.52 across the measured mixes. This report explicitly limits the ranking to these two engines and preserves the older four-engine pilot separately.

The latest same-session two-engine rerun, on main `20b257a`, is in `docs/analytical-benchmark/matched-vm-round3-v1/`: rdfdb 14.483 / 14.082 s versus DuckDB 113.992 / 112.898, all 240 requests qualified. The Linux lifetime regression is checked before timing. Cross-session ratio changes are not isolated component speedups; the paired table is the supported comparison.
