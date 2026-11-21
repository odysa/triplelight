# Blazegraph 2.1.5 embedded fixture qualification

Blazegraph 2.1.5 opened and reopened a local DiskRW journal through `BigdataSailRepository` on Linux ARM64 with Temurin 21.0.13+9. Both profiles preserved the nine-triple count. Turning off XSD datatype literal inlining preserved all nine exact RDF terms in both direct iteration and SPARQL SELECT. With the default XSD inlining, the parser emitted the exact fixture, then the store returned `"01"^^xsd:int` as `"1"^^xsd:int`. That was the only lexical difference in the fixture.

| XSD inlining | Parser output | Direct read and SELECT after load | Fresh-process reopen |
|---|---|---|---|
| Default, on | Nine exact reference triples | Nine triples and nine bindings, one integer lexical change | Same count and lexical difference |
| Disabled | Nine exact reference triples | Nine exact triples and bindings | Nine exact triples and bindings |

Both profiles used an explicit-only triples repository with `NoAxioms`, truth maintenance off, and text indexing and statement identifiers disabled. The parser's datatype normalization setting was false. The open repositories reported text inlining off, date-time inlining on, maximum inline string length zero, and the BNode property on. The profiles differed only in the XSD datatype-literal override. The fixture exercised no date-time or blank-node terms.

All four observations returned the SELECT projection `o`, zero rows for `FILTER(false)`, and false for the absent-subject ASK. Both loads recorded commit and shutdown returning. Each fresh process opened the exact 10 MiB journal left by its loader. The loaded and reopened bags gave matching observations within each profile. The journal digests also stayed unchanged through reopen in this run.

The harness used Maven Central's published `com.blazegraph:blazegraph-jar:2.1.5`, SHA-256 `8f38b1dae5ad92b0c786b92d1f260a18a4ca64151b62c8d5017dd59b743e131e`. It verified the publisher SHA-1, archive size, manifest and six sample class digests before compilation. Storage ran in-process with no server endpoint or extra JVM module flags. These observations cover the selected fixture on the pinned release. No workload timing was collected. [Maven Central artifact](https://central.sonatype.com/artifact/com.blazegraph/blazegraph-jar/2.1.5) · [Publisher's embedded example](https://github.com/blazegraph/database/wiki/First_Application_Tutorial)

## Replay

Extract `packet.tar.gz` into a fresh directory. From that directory run:

```bash
python3 -B replay.py . > audit.json
cmp audit.json AUDIT.json
```

The replay needs Python 3.11 or newer and uses the standard library. It compares the raw typed observations to the retained N-Triples fixture and independent Jena reference. `PACKET_FILES.json` lists every archived digest. The jar and private journals remain in the run directory. The packet retains their identity receipts and the read and query observations.
