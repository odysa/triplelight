# Grafeo 0.6.3 embedded Python fixture

Grafeo passed all **24 typed SPARQL checks** on macOS ARM64 and CPython 3.14.3. The fixture ran in memory, in the process that loaded a fresh persistent store, and in a separate process that reopened the store without reloading the fixture.

| Boundary | Triples added by `load_rdf` | SPARQL COUNT | Typed checks |
|---|---:|---:|---:|
| In memory | 9 | 9 | 8/8 |
| Persistent store in the loader process | 9 | 9 | 8/8 |
| Fresh process reopening the store | — | 9 | 8/8 |

The checks compare exact typed answers with the Jena reference. They include plain, DISTINCT, grouping, COUNT, COUNT(DISTINCT), an empty projection, an unbound variable and false ASK. Grafeo preserved the IRI and the literal with lexical text `urn:iri` as distinct terms. The pinned Grafeo and Solvor wheels passed `pip check`. Installed-file hashes matched before and after the fixture. [Grafeo wheel](https://pypi.org/project/grafeo/0.6.3/) · [Tagged Grafeo source](https://github.com/GrafeoDB/grafeo/tree/aa42b90d)

The closed store contained one 65,536-byte `fixture.grafeo` file. Its hash matched after the load and reopen closes. Grafeo reported WAL enabled and zero WAL records at both persistent opens. This run exercised clean close and reopen. The `info()` counters describe the property graph, so the table uses the returned RDF load counts and SPARQL answers. The nine-triple fixture checks correctness at this size. No workload timing was collected.

An earlier harness revision passed the eight memory checks and stopped before opening a disk store because it passed a `Path` to the constructor’s string parameter. The final run converted the path to a string and used a fresh directory.

## Replay

Verify `packet.tar.gz` against `PACKET_FILES.json` and extract into a fresh directory. From there, run `python3 -I -B replay.py`. It verifies the frozen source, runtime identities and store hash, checks the recorded typed answers, and reproduces `AUDIT.json` byte for byte. The packet retains the observations, tiny store and earlier constructor failure.
