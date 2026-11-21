# dotNetRDF 3.5.2 embedded .NET fixture

dotNetRDF passed **24/24 typed SPARQL checks** on macOS ARM64 using .NET SDK 10.0.404 and runtime 10.0.15. The RDF 1.1 N-Triples parser emitted all nine exact fixture triples. The native memory graph, the graph written through the file wrapper, and a fresh process that reparsed that file each reported nine exact triples and length nine.

| Boundary | Exact native triples | Typed query checks |
|---|---:|---:|
| In-memory graph | 9 | 8/8 |
| Graph written with FileGraphPersistenceWrapper | 9 | 8/8 |
| Fresh process reparsing the written graph | 9 | 8/8 |

Each route used an unnamed default graph and the native Leviathan evaluator. Literal normalization was disabled on every parser route. The checks cover grouping, plain projection, DISTINCT, both integer counts, a zero-row projection, an unbound variable and false ASK. The IRI and literal with lexical text `urn:iri` remained distinct.

The file wrapper wrote a 526-byte N-Triples serialization. Its bytes matched the input after flush, after dispose and after fresh-process reparse. The file route is a serialized graph round trip. The nine-triple fixture checks correctness at this size. No workload timing was collected. [Pinned dotNetRDF source](https://github.com/dotnetrdf/dotnetrdf/tree/67142d12e67fb68b37d009c3579ddebaeb4ff2c1)

The reviewed package lock matched across both setup attempts. The final build restored from the locked local feed and compiled with restore disabled. The first build stopped before running the engine because the harness omitted a required C# override. The online restore reported an advisory for the transitive AngleSharp 1.4.0 HTML parser. The fixture uses local N-Triples and SPARQL. [AngleSharp advisory](https://github.com/advisories/GHSA-pgww-w46g-26qg)

## Replay

Verify `packet.tar.gz` against `PACKET_FILES.json`, extract into a fresh directory and run `python3 -I -B replay.py` there. It checks the frozen source, package lock, runtime receipts, native observations and serialized graph, and reproduces `AUDIT.json` byte for byte. The SDK, package archives and binaries are identified by their recorded hashes.
