# Curated aggregate cohort: preparation and frozen comparison rules

This registers 72 COUNT, COUNT(DISTINCT) and GROUP BY queries over LUBM-100 and WatDiv100 before this stage collects new real-data responses. All 72 query files and scoped SQL translations are hash-bound, with both input file identities, validator/loader sources and tests. This cohort is curated from prior count-pushdown work and previously inspected small fixtures; it is not blind.

Rules freeze SHA-256: `5cf92a76e599d1e8a1db1f56a3f5215c3b05fea989665754c02ce3f76bb7f365`. The rules were announced in task #25 message a51d8b27 before new responses.

The primary gate compares complete RDF-term bags, canonicalizing only legacy typed-literal JSON spelling and the simple/xsd:string representation as the existing exact-term helper does. A separate COUNT-compatible gate permits exact nonnegative integer values in declared computed COUNT columns using xsd:integer, xsd:int or xsd:long. Datatype ranges and XML integer lexical forms are validated. Group keys, unbound state and multiplicity retain term identity. Floating counts and numeric tolerances are rejected. Independent reference COUNT outputs must use xsd:integer.

The new aggregate_count_policy.py is separate from the frozen BSBM policy. Twelve unit tests passed, including arbitrary precision, datatype ranges, invalid lexical forms, group identity, multiplicity, unbound outputs and projection shape. unit-tests.json records the observed execution; it is not a captured stdout transcript. The earlier scoped SQL translator has independent tiny-fixture validation in ../aggregate-sql-fixture-v1.

No real-data query results, competitor eligibility, import comparisons or performance results are established here. Independent Jena references and their separate hash binding come next. Any timing protocol must be registered separately. Full dataset/engine totals require every requested query to pass the selected gate; failed or unsupported cases remain visible.

The evidence directory preserves the frozen workload, translations, input identities and exact sources. No database files or shared input copies are included. SHA256SUMS.json hashes every report/evidence file except itself.
