# Native LadybugDB comparison

Completed LUBM-100 and BSBM-10M cohorts, including the paired tuned LUBM
schema, are recorded in `docs/ladybug-benchmark/README.md`. LadybugDB was confirmed by the
requester (the original request said Ladybird).

Pin `ladybug==0.22.3`. Its native Cypher executor is used directly, not the
separate `ladybug-rdflib` proof-of-concept adapter. The latter delegates SPARQL
evaluation through RDFLib and would measure a different stack.

## Primary mapping

`Resource` nodes represent IRIs/blank nodes; `Literal` nodes represent complete
RDF literal identities. Both have an integer primary key and a reversible
SPARQL-JSON term property. Literal columns also hold lexical value, datatype,
language, and a numeric projection. Plain literals and xsd:string are identical;
language tags are lowercased. Different numeric lexical forms and datatypes
remain distinct nodes. Resource/literal IDs share one unique ID space.

Each predicate gets separate typed relationship tables for resource and literal
objects, with native `Resource -> Resource` / `Resource -> Literal` endpoints.
Every distinct input RDF triple becomes one relationship, including rdf:type.
All triples are loaded; this is not a query-specific subset. External sort
deduplicates edges before COPY, reflecting RDF graph set semantics. Source line,
distinct relationship, and typed node counts are recorded and checked after COPY.
Node counts are supplemental dictionary entries, not additional RDF triples.

This schema-free mapping is the primary like-for-like comparison approved by
Kaiming. It is not a claim of optimal hand-designed Ladybug schemas. A separate
class-table/typed-column mapping, if measured, must be labeled separately and
preserve multi-valued properties/types.

The translator is deliberately limited. It accepts BGPs, OPTIONAL, the numeric,
IRI inequality, language and fixed-format date filters used by this cohort,
DISTINCT, ordering and limits. Unsupported syntax is reported per query, never
silently dropped. The two UNION shapes in the BSBM cohort, including its incoming/outgoing
variable-predicate query, have dedicated native translations. Other unsupported
shapes fail closed; this is not a general SPARQL implementation. It retains bag
multiplicity and full literal identity on output. Numeric projections reject non-finite values and integer values outside exact
DOUBLE range; dateTime comparisons require fixed timezone-free ISO timestamps
and matching datatypes. Native fixtures and full cohort answers are checked.

## Method

Start on host LUBM-100 and BSBM-10M; same-session sequential runs of Jena reference,
Ladybug, rdfdb memory and rdfdb disk. Match thread and cache settings where their
interfaces permit; record actual settings and limits (a cache target is not a
hard total-RSS limit). Record RDF conversion separately from native COPY load,
and both their sum and peak RSS. The HTTP adapter measures complete result
materialization/serialization with the same client boundary as rdfdb; this
includes Python adapter overhead and is not a pure engine-kernel benchmark.

Hash the complete canonical projected result multiset, preserving duplicates,
on every run. A mismatch disqualifies that query from performance summaries.
No truncated/unsupported query counts as a win. Preserve raw times, query/source
hashes, binaries/package provenance and the input hash. At least one warm-up
plus five measured runs per query; a single session remains a limited sample.

Commands (exclusive shared-machine benchmark lock required):

```sh
python native.py prepare INPUT.nt CSV_DIR
python native.py load CSV_DIR DB_PATH --memory-mb 10240 --threads 4
python native.py compile CSV_DIR ../lubm COMPILED.json
python native.py serve DB_PATH COMPILED.json --port 7981 --memory-mb 10240 --threads 4
python test_native.py
```

Sources: [LadybugDB](https://github.com/LadybugDB/ladybug),
[releases](https://github.com/LadybugDB/ladybug/releases/tag/v0.22.3),
[separate RDFLib adapter](https://github.com/LadybugDB/ladybug-rdflib).

## Stronger native execution configuration

The final cohorts use `--output-mode arrow` and `--plan-mode parameterized`
on the native service. Bulk output constructs JSON from complete pre-encoded
RDF-term properties; parameterized `Connection.execute` uses the driver’s
prepared-plan cache. The adapter does not retain answers. Raw getters include
the prepared plan’s original compilation time, which is not repeated work.

`compare.py --ladybug-output arrow --ladybug-plan-mode parameterized` runs
this configuration, with `--tuned-database` / `--tuned-compiled` adding the
paired tuned schema. The tuned mapper uses exact explicit-type signatures
and Parquet list columns, preserves multiply typed resources and multi-valued
properties, and currently accepts only the plain LUBM BGP cohort.

The first row-adapter/compile-per-request result remains historical evidence;
use the final paired/native-parameterized tables for the headline comparison.
