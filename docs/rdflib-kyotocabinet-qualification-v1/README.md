# RDFLib Kyoto Cabinet packaging and historical interoperability

The unmodified Kyoto Cabinet adapter failed to build on CPython 3.12.16 with modern setuptools. A separate historical RDFLib 4.2.2 profile completed the nine-triple fixture on CPython 3.9.23. The declared `Kyoto_Cabinet` dependency was unavailable from PyPI. The historical profile used the Debian Python package to provide the native binding, without Python distribution metadata for that dependency.

## Modern packaging

The untouched adapter source at `c65bf64f278078b9fb6d1242b5dadced77d3ef28` failed in wheel metadata generation under ARM64 Linux and setuptools 80.9.0. Its setup calls `distutils.util.run_2to3`, which this toolchain lacks. The stage stopped before adapter import and RDF access. The runtime behavior of this adapter with a modern RDFLib was untested. [Pinned adapter source](https://github.com/RDFLib/rdflib-kyotocabinet/tree/c65bf64f278078b9fb6d1242b5dadced77d3ef28)

## Historical interoperability

This archival result applies to RDFLib 4.2.2 only. Current deployment support and comparisons with the RDFLib 7.6 storage profiles are outside its scope.

The historical profile used ARM64 Linux, CPython 3.9.23, RDFLib 4.2.2 and the Debian Python binding 1.22-3+b3 with the Kyoto Cabinet 1.2.76-4.2+b1 library. Setuptools 57.5.0 used the source's built-in Python 3 conversion. The Debian binding's ABI tag matched the interpreter. Setup and each storage worker verified the private native extension, Kyoto Cabinet library and LZO library. [Debian Python binding](https://packages.debian.org/bullseye/python3-kyotocabinet) · [RDFLib 4.2.2](https://pypi.org/project/rdflib/4.2.2/)

| RDFLib setting | In-memory parse | Load | Fresh-process reopen |
| --- | --- | --- | --- |
| Default normalization | 9 terms with two lexical changes | Graph length 9, exact match to parsed bag | Graph length 9, exact match to parsed bag |
| Normalization disabled | 9 exact reference terms | Graph length 9, exact reference bag | Graph length 9, exact reference bag |

The first observed changes appear during the default RDFLib parse. `"01"^^xsd:int` was read as `"1"^^xsd:int`, and `"INF"^^xsd:double` as `"inf"^^xsd:double`. Both Kyoto Cabinet storage reads retained the parsed bag in each profile. Each profile closed cleanly, then a fresh process reopened the same database with creation disabled. Eight `.kch` files were inventoried after each boundary.

The separate historical RDFLib evaluator diagnostic returned nine rows for `FILTER(false)` at all four storage observations, against zero expected. This run covers historical packaging and fixture fidelity. It collected no workload timing.

The first historical attempt built the adapter but failed to load LZO because the harness omitted one of the private Debian library directories. The retry added the directory from the pinned ELF paths. No RDF data was opened on the failed attempt. The packet retains both historical attempts and the modern packaging control.

## Replay

Extract `packet.tar.gz` into a fresh directory. From that directory run:

```bash
python3 -B replay.py . > audit.json
cmp audit.json AUDIT.json
```

`PACKET_FILES.json` lists all archived digests. The packet includes both frozen harness revisions, packaging and runtime receipts, typed observations and database file inventories. The private database files remain in their run directories. Replay compares the retained observations to the independent N-Triples reference without starting an RDF engine.
