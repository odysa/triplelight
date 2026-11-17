# Cross-engine RDF mapping checks

An RDF-to-property-graph benchmark must conserve facts and query semantics, not
just successfully import a file. Keep the raw RDF line count, the distinct
triple count, and the mapping-specific conservation equation separate.

- With explicit predicate edges, every distinct RDF triple is one relationship.
  Resource/literal nodes are dictionary entries, not additional RDF statements.
- With class tables and property columns, count explicit type facts, relationship
  facts and property-list elements. A resource with multiple RDF types must not
  silently become one arbitrarily chosen type; exact type-signature tables are
  one way to preserve those facts. Multi-valued properties need lists or edges.
- Keep literal lexical form, datatype and language identity, even when storing
  an additional numeric projection for value comparisons. DISTINCT on RDF terms
  differs from DISTINCT on numeric values.
- Verify complete result bags, including duplicate projected rows and unbound
  OPTIONAL values, against a reference before reporting timings. Query agreement
  is scoped evidence, not a proof that every possible query is supported.

In the LadybugDB 0.22.3 experiment, our CSV transport of `STRING[]` values
containing serialized RDF-term JSON was invalid: three logical property values
became six list elements because of internal commas. The native import accepted
the file, but the fact-conservation fixture rejected it. Native Parquet list
columns preserved both the element counts and full literal identities. Treat
this as a finding about that particular transport representation, not a claim
that every Ladybug CSV list is broken. Keep the failing fixture/logs and validate
the actual native reader rather than assuming a Python serialization roundtrip
tests the database importer.

Compare identical timing boundaries. A native query's engine-reported time
cannot be compared directly with another system's complete HTTP response time.
Keep engine telemetry supplementary, and include adapter/materialization costs
in the application-level comparison. Report conversion separately from native
COPY, and include both when stating end-to-end import cost.
