# RDFLib-HDT read qualification

RDFLib-HDT 3.3 opened an HDT file written by Jena/HDT. In two fresh processes the native reader and RDFLib Graph each counted 11 triples across the full file. The native string iterator returned all nine exact terms in the selected subgraph. RDFLib's typed iterator returned nine terms in both processes and matched the reference with literal normalization disabled.

| Reader profile | Native total across file | Graph length across file | Native strings in subgraph | Typed RDFLib terms in subgraph |
| --- | ---: | ---: | --- | --- |
| Default normalization | 11 | 11 | 9, exact | 9, two lexical forms changed |
| Normalization disabled | 11 | 11 | 9, exact | 9, exact |

With normalization enabled, the typed reader returned `"01"^^xsd:int` as `"1"^^xsd:int` and `"INF"^^xsd:double` as `"inf"^^xsd:double`. The raw strings from the same native document retained both lexical forms. The replay treats untyped strings and explicit `xsd:string` as the same RDF term and compares multisets. Its token parser covers the fixture syntax.

Both processes used the memory-mapped, indexed reader with safe mode disabled. Each opened its own read-only copy of the 2,035-byte HDT file. Both HDT copies kept their original SHA-256. Each reader generated a 112-byte index sidecar. The sidecars were byte-identical. The HDT file originated in the separate Jena 4.3.2 and HDT 3.0.10 qualification. This run establishes read interoperability for that file.

The shared RDFLib evaluator returned nine rows in each process for the separate `FILTER(false)` diagnostic, against zero expected. The optional global BGP optimizer remained disabled. The fixture test covered reads and term fidelity. It collected no workload timing and made no write or broader SPARQL qualification.

The pinned environment used the released RDFLib-HDT 3.3 universal2 wheel and RDFLib 7.6.0 on macOS ARM64 with Xcode CPython 3.9.6. Setup verified 273 installed files before opening the dataset. The first harness attempt stopped before installation because Python 3.9 rejected a keyword on `Path.stat`. The reviewed revision used `Path.lstat()`, kept the same fixture and wheel pins, and completed both readers. The packet retains the first failure and both lock releases. [RDFLib-HDT 3.3 release](https://pypi.org/project/rdflib-hdt/3.3/)

## Replay

Extract `packet.tar.gz` into a fresh directory. From that directory run:

```bash
python3 -B replay.py . > audit.json
cmp audit.json AUDIT.json
```

`PACKET_FILES.json` lists every archived digest. The packet includes the frozen harness and fixture, runtime manifest, observations, both index sidecars and the independent replay. The raw runtime records include the private local run paths needed to trace the original execution.
