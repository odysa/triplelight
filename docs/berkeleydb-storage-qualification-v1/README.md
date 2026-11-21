# RDFLib BerkeleyDB storage qualification

With RDFLib literal normalization disabled, BerkeleyDB returned all nine triples with the expected RDF term identities, both after loading and after reopening in a fresh process. The default profile also returned nine triples at both boundaries, with two changed lexical forms. Both profiles used the same N-Triples fixture and frozen Jena reference.

| RDFLib parser setting | Scoped Graph length at load and reopen | Direct storage triple bag at load and reopen |
| --- | --- | --- |
| Installed normalization default | 9 at both boundaries | The two numeric literals `"01"^^xsd:int` and `"INF"^^xsd:double` came back as `"1"^^xsd:int` and `"inf"^^xsd:double`. The separate `"1"^^xsd:integer` remained present. |
| Normalization disabled | 9 at both boundaries | Matches the reference, including both original numeric lexical forms. |

The default-profile differences are consistent with the RDFLib N-Triples parser normalization predicted before the run. For both profiles, the load and reopen reads agreed and every recorded close returned without error. The inventory between the two processes contains three nonempty indexes and Berkeley DB region files. No log file was observed. The pinned plugin uses Berkeley DB's Concurrent Data Store mode with a memory pool and without requesting transactional recovery. The run checks ordinary reopen on this fixture.

The separate `FILTER(false)` diagnostic returned nine rows in every observation, against zero expected. It was run through RDFLib's shared SPARQL evaluator on purpose, and the same result was observed in the earlier Memory profile. The storage verdict uses the Graph length and the typed triples read directly through the storage API. This qualification carries no SPARQL workload admission or timing. [Earlier RDFLib Memory qualification](../rdflib-tiny-qualification-v1/README.md)

The test used RDFLib 7.6.0, berkeleydb 18.1.15, Berkeley DB 5.3.28 and CPython 3.14.3 on macOS ARM64. The Python binding was built offline against a privately extracted Homebrew library. Each worker verified the mapped library and binding against the setup receipt. The original and re-signed library digests, build source identifiers, installed-file identities and observations are retained in the packet.

## Replay

Extract `packet.tar.gz` into a fresh directory. `PACKET_FILES.json` lists the SHA-256 digest of every archived file. From the extracted directory, run:

```bash
python3 -B replay.py . > audit.json
cmp audit.json AUDIT.json
```

The packet includes the four native observations, the between-process file inventories, the frozen source and fixture, and the setup identity. The original Berkeley DB stores remain in the run directory.
