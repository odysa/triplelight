# Ontop 5.5.0 with DuckDB JDBC: tiny qualification

**This configuration fails the declared tiny admission gate.** One untimed attempt completed all 13 fixed queries: twelve pass, while `COUNT(DISTINCT ?o)` returns `"8"^^xsd:integer` where the frozen expected answer is `"9"^^xsd:integer`. This is a count-value mismatch, not merely a datatype or projection mismatch. Plain `COUNT(*)` returns the expected integer 9. No workload timings or full-workload admission follow.

| Fixed case | Result |
|---|---|
| Grouped strict object/count bag | pass |
| Plain strict object bag | pass |
| DISTINCT strict object bag | pass |
| COUNT(*) | pass: integer 9 |
| COUNT(DISTINCT ?o) | fail: integer 8, expected integer 9 |
| Empty projected result | pass |
| OPTIONAL object bag and missing unbound cells | pass |
| False native ASK | pass |
| sameTerm IRI/string/language filters (three cases) | all pass |
| xsd:int datatype filter | pass |
| isIRI filter | pass |

The SQL source contains eleven rows: nine distinct RDF objects, one duplicate IRI row and one NULL IRI row. Eight static R2RML mappings construct the graph; the native grouped, plain and DISTINCT answers each retain all nine distinct objects. The NULL row yields no object triple, and the repeated IRI yields one graph triple. The fixture distinguishes IRI from identical string text, language, numeric datatype and lexical form, empty text, boolean, infinity and decimal. Grouping preserves `"01"^^xsd:int`, `"1"^^xsd:integer` and `"INF"^^xsd:double`. These observations do not identify why COUNT(DISTINCT) returns eight. No query or mapping was rewritten after seeing the result, and no retry was made.

RDF terms and projections were obtained from the in-process RDF4J Repository result API. The adapter does not use Ontop CLI CSV, insert expected result metadata into answers or invoke a TripleLight evaluator. Mappings explicitly declare RDF datatypes; this tests construction and evaluation under those declarations, not inferred SQL-to-RDF types. No blank nodes appear. The tiny result is specific to this configuration, not a claim about other Ontop versions, SQL backends or arbitrary queries.

The isolated runtime was actual Temurin Java 25.0.5.1+2 on macOS arm64, Ontop 5.5.0 and DuckDB JDBC 1.5.6.0. Download digests are in the archived pin files; RUNTIME.json lists 219 unique files, including the Java executable, JDK release/libjvm/modules, compiled adapter and 172 explicitly enumerated classpath jars. These identities were checked before and after execution. A first compile failed on an ambiguous Statement type; that source and logs are retained. Qualifying java.sql.Statement allowed the second compile. Separate Java 25 JSON serializer controls passed before any mapping query. These setup checks are not query observations.

Source freeze: `4378cf865ef308885a1ae6b10db29168fc1f9b34cede00336e5ef393513060b2`. Prospective execution record: `fb36f9ec831ae3aa11749e746bac27eba4e7aebfa01c308a167906ab459cf431`. One 120-second child cap covered JDBC setup and all thirteen cases; the JVM heap was capped at 2 GiB, with no whole-process memory or CPU limit. The child and supervisor exited zero, no timeout or identity drift occurred, and the exact owned lock was released after exit. Successful process exit does not mean answer admission.

The 61-file packet includes reviewed source, mappings, SQL input, fixed queries and Jena reference, native typed answers, runtime/preparation manifests and setup/query receipts. Installed JDK/jars, compiled binaries and the generated DuckDB file are excluded. The portable replay verifies packet/source identities and recomputes the thirteen verdicts without importing an engine or executing archived source. Runtime manifests document the installed environment but cannot reconstruct or independently rehash omitted binaries. The broader SQL-mapping and embedded-store comparison remains open.
