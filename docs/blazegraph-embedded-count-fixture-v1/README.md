# Blazegraph 2.1.5 embedded COUNT fixture

With Blazegraph 2.1.5 on Linux ARM64 and Temurin 21.0.13+9, an explicit-only `COUNT(*)` over one triple pattern failed in the fast range-count operator. The same query returned `"9"^^xsd:integer` when `includeInferred=true` on a store with no axioms and truth maintenance off. Both exact counts in that query process were nine, so the fixture contained only explicit statements. No large dataset ran in these phases.

| Profile | Total at load / reopen | Typed SELECT | COUNT(*) | FILTER(false) |
|---|---:|---|---|---|
| XSD inlining disabled, explicit-only | 9 / 9 | Nine exact terms | QueryEvaluationException with a nested AssertionError in FastRangeCountOp | Zero rows, projection o |
| XSD inlining disabled, includeInferred=true | 9 / 9 | Nine exact terms | `"9"^^xsd:integer`, projection count_result | Zero rows, projection o |
| Default XSD inlining, explicit-only | 9 / 9 | Nine terms, one known integer lexical change | Same FastRangeCountOp assertion | Zero rows, projection o |

The primary streaming fixture used the pinned Maven jar, triples-only DiskRW, a 10,000-statement buffer, queue capacity zero, parser datatype normalization off and one explicit commit. A new JVM checked the exact reopened count before a third JVM ran the query fixture. The two Java 21 controls used the same fixture and SPARQL bytes. They used the same diagnostic class bytes and separately changed the query scope or the inline setting against the primary fixture. The diagnostic worker also recorded the exact total and explicit-only counts after its queries.

The failure stack ends at the branch that rejects an index-local filter. The AST translation adds an explicit-statement filter for `includeInferred=false`. The passed all-statements control and the repeated failure with default inlining support this explanation for the tested route. The fixture does not establish that every COUNT query uses this route. [Branch that throws](https://github.com/blazegraph/database/blob/cb089919/bigdata-core/bigdata/src/java/com/bigdata/bop/join/FastRangeCountOp.java#L371-L381) · [Explicit-statement filter](https://github.com/blazegraph/database/blob/cb089919/bigdata-core/bigdata-rdf/src/java/com/bigdata/rdf/sparql/ast/eval/AST2BOpUtility.java#L5253-L5259)

The planned Java 17 control stopped during archive extraction. The host disk sampler followed a licence symlink whose target had yet to be extracted. No Java 17 process ran. The report retains that harness failure and makes no Java 17 compatibility claim.

The explicit-only Java 21 configuration stopped at the fixture gate before LUBM-100 import. A separate workload registration can use the all-statements query setting with inference disabled and exact total and explicit counts required at load and reopen. This packet contains no workload timing.

## Replay

`packet.tar.gz` contains the raw typed results, stack logs, stage and journal receipts, pinned source and the engine-free replay. The baseline packet includes the fixture-related portion of its 83-file freeze. The diagnostic freeze is included in full. The JDK archive and database journals are omitted, with identities or receipts retained. Verify member digests against `PACKET_FILES.json`, extract the archive into an empty directory and run `python3 -I -B replay.py` from that directory. Its output equals the saved `AUDIT.json`.
