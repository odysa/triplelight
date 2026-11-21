# sparq-rdf 0.1.8 embedded Python fixture

sparq-rdf passed all **24 checks** on macOS ARM64 and CPython 3.14.3. A memory graph, a separately loaded graph saved to disk, and a fresh read-only reopen each reported a native length of nine and passed eight typed SPARQL checks. The read-only reopen left the 17-file inventory byte-identical.

| Boundary | Native length | Typed query checks |
|---|---:|---:|
| In-memory graph | 9 | 8/8 |
| Fresh graph after save | 9 | 8/8 |
| Fresh process, read-only indexed reopen | 9 | 8/8 |

The pinned ARM64 wheel was installed in a private environment. pip check passed, and the installed-file hashes were unchanged across the fixture. The worker used the native Python `Graph.load`, `Graph.save`, `Graph.open_read_only`, `Graph.query` and `Graph.ask` APIs. The save-stage queries ran on the graph in the loader process. Only the final stage queried the reopened index. The 17-file inventory covers the saved permutation indexes and dictionary, sidecar metadata and lock files.

The checks compare the exact Jena reference for group, plain and DISTINCT results, both integer counts, empty projection metadata, an unbound binding and false ASK. Native terms preserve both the IRI and literal with lexical text `urn:iri` as different kinds. sparq uses OxRDF term types and its own query evaluator. The Python loader reads the full input into a string before building the graph, which needs a separate memory evaluation at scale. [Published wheel](https://pypi.org/project/sparq-rdf/0.1.8/) · [Tagged Python implementation](https://github.com/sparq-org/sparq/blob/f52400472814b004ca7339ec0cbfc641b16e4c1a/crates/sparq-py/src/lib.rs)

These are selected fixture and clean-reopen checks. No workload timing was collected.

## Replay

Verify `packet.tar.gz` against `PACKET_FILES.json` and extract into a fresh directory. Run `python3 -I -B replay.py` from there. It replays the native records, verifies the preserved index against the pre-reopen inventory and reproduces `AUDIT.json` byte for byte. The packet retains the source, runtime receipts and tiny store. The wheel is identified by its checksum.
