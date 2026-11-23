# Embedded RDF stores: evidence inventory and remaining work

This inventory links the measured embedded cohorts and the smaller installation, storage and answer checks.
A workload enters the timing tables after its declared answer checks pass. Each entry identifies the runtime,
storage mode and limits of the evidence.

## Completed evidence

| Candidate/configuration | Actual evidence | Remaining limit |
|---|---|---|
| TripleLight Python disk / DuckDB typed-triples SQL | [Python cohort](https://app.raft.build/artifacts/Azoz2CgbhGKQtHczyXUoiY): all 1,104 scheduled requests pass selected gates across BI10M and aggregate LUBM/WatDiv; two registered passes. | Historical source e2e9cb2f, Python 3.11.18. DuckDB SQL is hand-authored BI/scoped aggregate translation, not R2RML. |
| PyOxigraph 0.5.13 disk | Same Python report: LUBM/WatDiv aggregate pairs admitted and scored. | BI qualification stopped at the 900-second whole-phase cap after 11/60 responses. Tiny nine-term fixture returned eight groups. No eligible BI total. |
| TripleLight Node disk / quadstore 15.4.2 + Comunica 6.3.1 + LevelDB | [Node cohort](https://app.raft.build/artifacts/BasQeph4reGyBoJpfZ69hE): completed v2 has 864 passing requests; quadstore WatDiv admitted/scored alongside TripleLight. | Quadstore BI/LUBM not admitted. The stopped v1 cap outcome and separately registered increased v2 cap remain disclosed. |
| Oxigraph JS 0.5.13 memory/WASM | Same Node report: LUBM/WatDiv admitted/scored as a separate memory group. | BI stopped at the 1,800-second whole-phase cap after 11/60 passing answers. Tiny term-identity limitation remains; projection columns in this adapter come from frozen references, not an independent empty-result API check. |
| Jena TDB2 6.2.0 | [Native Java cohort](https://app.raft.build/artifacts/4FiCDWgEyXpz3b3fRS8GLk): BI10M and both aggregate workloads scored. | Separate JVM session; no TripleLight Java binding evaluated in that cohort. Earlier tiny noncanonical-integer identity limitation is retained. |
| RDF4J 5.3.2 NativeStore / LMDBStore | Same JVM cohort: both aggregate workloads admitted/scored. | BI fails admission on 12 q5 instances per backend; no BI total. Native uses prepared clones; LMDB imports fresh each phase outside query time. |
| RDF4J 5.3.2 MemoryStore | Same JVM cohort: aggregate workloads admitted/scored in a separate memory group. | BI q5 admission failures; no memory/disk pooling. Whole JVM cohort has 1,272 passing collected answers. |
| HDT 3.0.10 + Jena 4.3.2 | [Standalone HDT report](https://app.raft.build/artifacts/Tbn4rJhupS6fZNUv9M8zb1): 408 passing requests across BI/LUBM/WatDiv and tested portable replay. | Compressed read-only triple storage with Jena query lineage, older Jena and separate session; not an independent SPARQL implementation or a TDB2 version comparison. |
| Oxiland 0.13.0 source eb5aa762, memory / Fjall | [Installation and qualification](https://app.raft.build/artifacts/8eNdfz3NnahN3zQZ3bRbHX): Linux ARM64 source build succeeds. Native iterator and serialized API tested. | Both modes return eight groups instead of nine and fail sameTerm; native iterator lacks empty-result projection metadata. No workload timing. Oxigraph query lineage. |
| Redland 1.0.17 / Rasqal 0.9.33 / Raptor 2.0.16, native C memory adapter | Linux ARM64 installation and nine-row manual smoke complete. Shared frozen group query yields four groups instead of Jena's nine in both sparql and sparql11-query. ASK controls pass. | Empty projection/unbound JSON limitations; evaluator versus serializer not isolated. No workload timing. Actual executable/library hashes preserved; apt repository snapshot was not pinned. |
| LadybugDB property-graph mapping | Historical mapped cohorts plus [new within-Ladybug generic/hybrid Explore comparison](https://app.raft.build/artifacts/8VcJUG98ks6qNp3y5YruCc). | SQL/property-graph mapping, not native RDF/SPARQL. New Explore mapping study has different scope and no new cross-engine comparison. |

The [JVM report and replay evidence](https://app.raft.build/artifacts/4FiCDWgEyXpz3b3fRS8GLk) are available separately; its registered freeze is f888a81224b063a2fdc98137a7b52961007f2c99ebd469e47118cd352a41a8df. This inventory copies no timing observations into a new session. All selected answer gates are bounded checks, not general conformance certification. Missing totals are not zeros or measured losses.

## Index layouts in the benchmarks

The same input and reference answers can be tested with different physical layouts. These rows distinguish
observed layouts from published product defaults.

| Profile | Layout evidence |
|---|---|
| TripleLight external disk, BSBM-100M | The RDFSNAP4 store contains six triple-order files and one unified dictionary. [Raw file lengths and build receipts](unified-dictionary-100m-v1/README.md) |
| DuckDB typed-triples SQL, disk Python and analytical HTTP | One triples table stores the subject, predicate and typed object struct. The loader creates no ART index. DuckDB builds automatic min-max zonemaps. [Python cohort](https://app.raft.build/artifacts/Azoz2CgbhGKQtHczyXUoiY) · [DuckDB index types](https://duckdb.org/docs/2.0/sql/indexes) |
| QLever, three retained preparation builds | All six main orders appear in the LUBM-100, WatDiv and BSBM BI 100M index inventories. Each also contains internal indexes and vocabularies. [File lengths and preparation intervals](benchmark-index-receipts.json) |
| Jena TDB2 6.2.0, native Java tiny fixture | The retained B+ tree filenames show SPO, POS and OSP for triples, and GSPO, GPOS, GOSP, SPOG, POSG and OSPG for quads. The observation applies to the tiny fixture. [File inventory](benchmark-index-receipts.json) |
| Oxigraph and PyOxigraph disk | The upstream RocksDB design documents SPO, POS and OSP for default-graph triples and six orders for named-graph quads. This describes the disk design. The in-memory backends use different structures. [Oxigraph architecture](https://github.com/oxigraph/oxigraph/wiki/Architecture#how-are-data-encoded-in-rocksdb) |
| Virtuoso 7 HTTP | The documented default has two full quad indexes, PSOG and POGS, and three partial indexes, SP, OP and GS. The benchmark receipts do not bind the full index catalog for each historical run. [Virtuoso index scheme](https://docs.openlinksw.com/virtuoso/rdfperfrdfscheme/) |
| RDF4J NativeStore | Its documented defaults are SPOC and POSC. The measured workload profile did not use the defaults: it created the store with the index list `spoc,posc,ospc`, three orders (receipt: `rows[3]` and `rows[4]`, constructor argument in `source/Rdf4jEngine.java`). The [native Java cohort](https://app.raft.build/artifacts/4FiCDWgEyXpz3b3fRS8GLk) also covers separate LMDBStore and MemoryStore profiles. [Receipt supplement](benchmark-index-receipts-supplement.json) (SHA-256 `05e17cc4…`) · [RDF4J storage configuration](https://rdf4j.org/documentation/reference/configuration/#native-store) |

The two RDFSNAP4 BSBM-100M builds have the same file lengths. Their six orders occupy 3,316,099,215 bytes,
the dictionary 4,620,210,974 bytes and the manifest 232 bytes. The preceding format used two dictionary files.
Across these builds, the store changed from 12,533,110,608 bytes (11.67 GiB) to 7,936,310,421 bytes (7.39 GiB),
a 36.7% reduction. The new builds took 706.3 s and 704.3 s. [Raw old build](unified-dictionary-100m-v1/raw/1-old.result.json)
· [Raw new builds](unified-dictionary-100m-v1/raw/2-new.result.json) and
[second new build](unified-dictionary-100m-v1/raw/3-new.result.json)

The planned DuckDB experiment compares the existing one-table layout with six sorted copies. A fixed-SPO arm
and a routed arm use the same six-copy database. Sorted DuckDB tables provide different access paths from
TripleLight's seekable indexes. The experiment is still in preparation.

## Existing evidence paths and provenance

The original working list and Redland raw evidence are retained in the reviewer's workspace. The first frozen-fixture attempt preserves a non-UTF-8 harness failure; the second contains the completed raw outputs, identities and checks. The executable/library manifest has SHA-256 92a868cfcfaf31ffa3b17da3f6d6e8225ca3628d28b740b029a07c6a5b7d5f3f. Those records were read without changing them. This reconciliation is not a new independent runtime replay of Redland.

Sources reconciled for this inventory are the linked Python, Node, JVM and HDT/Oxiland reports, plus the retained original working list and Redland records. Each report retains source/runtime/query/gate identities, failed admission history, timing boundaries and raw evidence. Runtime and storage groups stay separate.

## Remaining candidates and next actions

| Candidate | Established surface | Current status / next action |
|---|---|---|
| RDFLib 7.6.0 Memory | [Official stores documentation](https://rdflib.readthedocs.io/en/stable/apidocs/rdflib.plugins.stores/); [pinned tiny qualification](rdflib-tiny-qualification-v1/README.md). | macOS ARM64, CPython 3.14.3, pyparsing 3.3.3: default normalization rewrites tested int/double lexical forms; lexical preservation passes strict term bags. Both declared profiles return nine rows for fixed FILTER(false), expected zero, so neither passes all tiny controls. Projection metadata and false ASK pass. No workload timing or full-workload admission. |
| RDFLib BerkeleyDB | [Pinned storage qualification](berkeleydb-storage-qualification-v1/README.md), with RDFLib 7.6.0 and Berkeley DB 5.3.28 on macOS ARM64. | Both profiles return nine triples at load and reopen. Disabling RDFLib normalization preserves the exact reference terms. The default profile changes two numeric lexical forms. The separate shared RDFLib FILTER(false) diagnostic returns nine rows, expected zero. No workload timing. |
| Oxrdflib 0.5.0 / PyOxigraph 0.5.13 | [Oxigraph's RDFLib adapter](https://github.com/oxigraph/oxrdflib); [pinned disk qualification](oxrdflib-disk-qualification-v1/README.md). | macOS ARM64, CPython 3.14.3. All four parser and normalization profiles returned eight triples and eight groups at load and fresh-process reopen, against nine expected. The same noncanonical integer term is absent in each profile. No workload timing. |
| RDFLib SQLAlchemy / SQLite | [Pinned SQLite storage qualification](rdflib-sqlite-qualification-v1/README.md). | Original 0.5.4 stopped at import in its declared-dependency environment. With [explicit setuptools](rdflib-sqlite-upstream-qualification-v1/README.md), it reproduced the BrickSchema storage result. BrickSchema 0.7.1 with normalization disabled preserves all nine typed triples through reopen. Its default profile iterates nine triples with two changed lexical forms while Graph length reports eight. The shared RDFLib evaluator still fails the separate FILTER(false) diagnostic. No workload timing. |
| COTTAS / RDFLib | [Pinned adapter qualification](cottas-tiny-qualification-v1/README.md). | PyOxigraph streaming parse, Parquet file and raw COTTAS search retain all nine exact terms. The typed RDFLib reader matches them with normalization disabled. The shared FILTER(false) diagnostic returns nine rows rather than zero. No workload timing. |
| RDFLib-HDT 3.3 | [Pinned read qualification](rdflib-hdt-qualification-v1/README.md). | Both readers count 11 across the HDT file. Each returns nine exact native terms from the selected subgraph. The typed RDFLib reader matches those nine with normalization disabled. The separate shared evaluator diagnostic returns nine rows against zero expected. No workload timing. |
| RDFLib LevelDB plugin | [Pinned Linux ARM64 qualification](rdflib-leveldb-qualification-v1/README.md). | Both loads and fresh-process reopens retain nine triples. With normalization disabled, all nine match the exact reference. Default-mode lexical changes first appear at RDFLib parse. Each reopen records WAL recovery. No workload timing. |
| Blazegraph 2.1.5 embedded | [Pinned DiskRW journal qualification](blazegraph-embedded-qualification-v1/README.md), Linux ARM64 and Java 21. | Both profiles retain nine triples after load and fresh-process reopen. Disabling XSD inlining preserves all nine exact terms in direct iteration and SELECT. The default changes one integer lexical form. FILTER(false) and false ASK pass in both profiles. A follow-up [streaming COUNT fixture](blazegraph-embedded-count-fixture-v1/README.md) found an explicit-only fast range-count assertion. With no inference configured and both exact counts equal to nine, includeInferred=true returned the expected typed count. The separate [LUBM-100 qualification](blazegraph-lubm100-qualification-v1/README.md) passed all 30 typed answer checks after measuring 13,409,395 total and explicit triples at load and fresh reopen. No scored latency. |
| sparq-rdf 0.1.8 | [Pinned native Python fixture](sparq-embedded-qualification-v1/README.md), macOS ARM64 and CPython 3.14.3. | Native length nine and 8/8 typed checks for each of memory, save-stage and fresh read-only indexed reopen. All 17 indexed-store and lock files matched across reopen. Uses OxRDF term types with its own evaluator. The Python loader reads the full source into a string. No workload timing. |
| Grafeo 0.6.3 | [Pinned native Python fixture](grafeo-embedded-qualification-v1/README.md), macOS ARM64 and CPython 3.14.3. | Both loads added nine triples. Memory, persistent load and fresh-process reopen each passed 8/8 typed checks. The closed 64 KiB store file matched across reopen. WAL was enabled with zero records observed at reopen. No workload timing. |
| goRDFlib v0.10.5 | [Pinned native Go fixture](gordflib-embedded-qualification-v1/README.md), macOS ARM64 and Go 1.26.4. | Nine exact native triples and 8/8 typed checks in memory and on each load and reopen route for Badger v4.9.1 and modernc SQLite v1.46.1. The five Badger files and SQLite database matched after clean reopen. No workload timing. |
| dotNetRDF 3.5.2 | [Pinned native .NET fixture](dotnetrdf-embedded-qualification-v1/README.md), macOS ARM64 and .NET 10.0.15. | Nine exact triples and 8/8 typed checks in memory, after whole-graph file write and after a separate-process reparse. The 526-byte N-Triples output matched input byte for byte. Restore reported an AngleSharp HTML-parser advisory. No workload timing. |
| Mulgara 2.1.13 | [Pinned build and startup check](mulgara-embedded-startup-v1/README.md), macOS ARM64, Zulu 8u514 and Ant 1.9.7. The embedded JAR built successfully. | The local Database constructor failed under network denial when JOTM attempted an RMI bind. Fixture load, query checks and reopen were unreached. No workload timing. |
| GraphDB embedded | GraphDB 11.5 documents the embedded RDF4J runtime in its public Maven repository. [Vendor Maven documentation](https://graphdb.ontotext.com/documentation/11.5/maven-artifacts.html) | Licence-restricted. The embedded runtime requires a separate GraphDB licence. Excluded from the benchmark. Installation and qualification require an owner-provided licence. |
| RDFox embedded Java | RDFox documents an ARM64 Java API and a time-limited licence. [Vendor requirements](https://docs.oxfordsemantic.tech/7.7/features-and-requirements.html) | Licence-restricted. Evaluation terms limit benchmark publication. Excluded from the benchmark. Installation and qualification require an owner-provided licence. [Vendor evaluation terms](https://www.oxfordsemantic.tech/rdfox-evaluation-license) |
| Oxilite 0.10.0 | [Project source](https://github.com/Volland/oxilite) describes native SQLite storage, a SPARQL-to-SQL compiler and Oxigraph-family components/fallback evaluator. | [Pinned macOS ARM64 tiny qualification](oxilite-tiny-qualification-v1/README.md): all 24 observations pass across memory, disk-load and separate-process disk-reopen; native typed terms, counts, projections and unbound/ASK controls pass. All saved explain outputs report fully compiled SQL, a compiler diagnostic rather than runtime tracing. Bundled SQLite version unobserved. A separate [Linux ARM64 platform qualification](oxilite-linux-tiny-qualification-v1/README.md), CPython 3.11.18 in a pinned container, also passes all 24 tiny observations, with persisted data on a macOS virtiofs bind mount; the profiles are not pooled. A separate [LUBM-100 import attempt](oxilite-lubm100-resource-stop-v1/README.md) hit the 12 GiB memory limit before returning a triple count. Reopen and all 30 query labels were skipped. |
| Ontop / DuckDB JDBC | [Official connector documentation](https://ontop-vkg.org/guide/databases/duckdb.html) describes an established DuckDB mapping route. | [Pinned tiny qualification](ontop-tiny-qualification-v1/README.md): Ontop 5.5.0 / DuckDB JDBC 1.5.6.0 / actual Java 25.0.5.1+2 on macOS ARM64 completes 13 cases; twelve pass and COUNT(DISTINCT) returns integer 8, expected 9. Plain, DISTINCT and grouped bags preserve all nine terms. The [four-profile diagnostic](ontop-distinct-diagnostic-v3/README.md) shows that the reformulated count uses only the lexical column in this mapping. Subject and predicate positions rule out the illustrated IRI–literal collision in 35 of 36 aggregate labels with COUNT(DISTINCT). WatDiv `distinct_L2.rq` needs a mapped-domain check. The full mapping and workload timings are pending. Separate from the DuckDB Python extension; no SQLite support inference. |
| DuckDB RDF extension / experimental SQLite adapter | [Official extension page](https://duckdb.org/community_extensions/extensions/rdf); source/docs linked in task46 protocol. | [Thirteen-control qualification report](https://app.raft.build/artifacts/8NmNF3jSXiNRdSeFgUsmVg) now completed: plain projection and both counts lose one of nine terms; typed projection and DISTINCT discriminator preserve nine. SQLite has ten execution errors and three passing controls. No timing; failed v1 setup and the v2 lock/cache incident are retained. Task46 remains open for another qualified route. |

The initial candidate list was not exhaustive. A maintenance label alone is not an installation failure: additional historical libraries should be evaluated by version, API and distribution availability. Existing Virtuoso/QLever server cohorts do not supply embedded latency evidence. Further discovered plugins are recorded as candidates rather than being represented as tested engines.

## Historical adapter qualification

| Adapter | Modern packaging | Archival interoperability |
|---|---|---|
| [RDFLib Kyoto Cabinet](rdflib-kyotocabinet-qualification-v1/README.md) | The untouched source failed during wheel metadata generation on CPython 3.12.16 and setuptools 80.9.0. | RDFLib 4.2.2 on Linux ARM64 with a substituted Debian binding retains nine triples across load and reopen. Normalization disabled preserves the exact reference. Current deployment support and comparisons with RDFLib 7.6 are outside this archival profile's scope. No workload timing. |

## Work in preparation

- The [matched Java 25 LUBM-100 qualification](blazegraph-matched-java25-lubm100-v1/README.md)
  passed all 30 exact typed checks per engine after both Blazegraph and TripleLight reopened with
  13,409,395 triples. This follows the [nine-triple fixture](blazegraph-matched-java25-qualification-v1/README.md).
  Query timing requires a separate registration.
- The Ontop workload mapping will qualify the remaining aggregate and BI queries against their reference
  answers. The [distinct-count diagnostic](ontop-distinct-diagnostic-v3/README.md) explains why WatDiv
  `distinct_L2.rq` needs a separate domain check.
- The [Ontop production-schema qualification](ontop-production-view-v2/README.md) passed four
  required typed checks on a nine-row DuckDB fixture using a view that exposes the object fields as
  columns. Direct nested-field mapping stopped at initialization. The mixed-term distinct count
  returned 8 against a typed reference of 9. Workload admission and timing remain pending.
- The [LUBM-100 DuckDB layout qualification](duckdb-layout-lubm100-v1/README.md) passed all 90 typed
  checks across the single-table, fixed-SPO and routed arms. All six sorted tables reopened with
  13,409,395 rows and matched the control relation. WatDiv and BSBM BI qualification remain pending.
