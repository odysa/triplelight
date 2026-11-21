# COTTAS Python adapter qualification

The COTTAS writer preserved all nine fixture triples in a Parquet file. Two fresh processes opened the same file. Both returned nine raw triples and a Graph length of nine. With RDFLib normalization disabled, the typed Graph iteration also matched all nine reference terms. The default typed reader changed two numeric lexical forms.

| Boundary | Result |
| --- | --- |
| PyOxigraph streaming parser | Nine exact fixture terms, including `"01"^^xsd:int` and `"INF"^^xsd:double`. |
| Written Parquet and raw COTTAS search | The same nine encoded triples as the parser, in observed SPO file order. Parquet index metadata is `spo`. |
| RDFLib typed reader with default normalization | Nine triples, with `"01"^^xsd:int` returned as `"1"^^xsd:int` and `"INF"^^xsd:double` as `"inf"^^xsd:double`. |
| RDFLib typed reader with normalization disabled | Nine triples matching the exact typed reference bag. |

The released COTTAS writer streams `pyoxigraph.parse` output to DuckDB, then sorts and deduplicates rows before export. It uses no PyOxigraph Store for this path. The raw search results and file reads preserve the numeric lexical forms. The first observed changes appear in the default RDFLib typed reader, which uses `from_n3`. The artifact is 780 bytes and has the same SHA-256 before and after both readers. [Pinned Python package](https://pypi.org/project/pycottas/1.1.0/)

The nine stored object strings in file order follow. The replay treats plain strings as equivalent to explicit `xsd:string`, and its lexical reader covers the escapes in this fixture.

```text
""
"01"^^<http://www.w3.org/2001/XMLSchema#int>
"1"^^<http://www.w3.org/2001/XMLSchema#integer>
"1.25"^^<http://www.w3.org/2001/XMLSchema#decimal>
"INF"^^<http://www.w3.org/2001/XMLSchema#double>
"hello"@en
"true"^^<http://www.w3.org/2001/XMLSchema#boolean>
"urn:iri"
<urn:iri>
```

The separate `FILTER(false)` diagnostic returned nine rows in both reader processes against zero expected. It ran through the shared RDFLib evaluator and stays outside the storage verdict. No workload timing or broader SPARQL admission was collected.

The profile used pycottas 1.1.0, PyOxigraph 0.3.18, DuckDB 1.5.0 and RDFLib 7.6.0 on macOS ARM64 and CPython 3.12.12. The setup receipt and installed-file manifest are in the packet.

## Replay

Extract `packet.tar.gz` to a fresh directory, then run from that directory:

```bash
python3 -B replay.py . > audit.json
cmp audit.json AUDIT.json
```

`PACKET_FILES.json` lists every archived digest. The packet retains the released-source identities, exact producer file, raw parser and reader observations, and the replay output.
