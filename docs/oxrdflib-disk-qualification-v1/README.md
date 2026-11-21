# Oxrdflib disk qualification

None of the four Oxrdflib profiles passed the nine-term fixture on macOS ARM64. Each reported eight triples through RDFLib's Graph length API and eight groups through SPARQL, both before close and after reopening in a fresh process. The frozen Jena reference has nine groups.

| N-Triples parser | RDFLib literal normalization | Graph length and group count at load and reopen | Missing typed object | Other lexical difference |
| --- | --- | --- | --- | --- |
| RDFLib `nt` | Installed default | 8 at both boundaries | `"01"^^xsd:int` | `"INF"^^xsd:double` returned as `"inf"^^xsd:double` |
| RDFLib `nt` | Disabled | 8 at both boundaries | `"01"^^xsd:int` | None observed |
| Oxrdflib `ox-nt` | Installed default | 8 at both boundaries | `"01"^^xsd:int` | `"INF"^^xsd:double` returned as `"inf"^^xsd:double` |
| Oxrdflib `ox-nt` | Disabled | 8 at both boundaries | `"01"^^xsd:int` | None observed |

The separate `"1"^^xsd:integer` reference term was returned by every profile, with group count one. The grouping and OPTIONAL queries returned the same object bag in each observation. All eight observations passed the result-column, empty-result, unbound-cell, false ASK and store-routing checks. Each failed the three checks for graph length, grouped typed terms and the unbound query's typed object bag.

The pinned adapter computes Graph length with a PyOxigraph query scoped to the named graph and reads the scalar directly, before RDFLib result-term conversion. The length of eight, including after direct `ox-nt` ingestion with RDFLib normalization disabled, shows that result conversion alone cannot account for the cardinality difference. The earlier direct PyOxigraph 0.5.13 qualification also reported eight groups on the nine-term fixture. These runs used different API and platform profiles. The present evidence leaves the exact storage or execution mechanism unobserved. [Earlier Python comparison](https://app.raft.build/artifacts/Azoz2CgbhGKQtHczyXUoiY)

The run used CPython 3.14.3, Oxrdflib 0.5.0, PyOxigraph 0.5.13, RDFLib 7.6.0 and pyparsing 3.3.3. It used an explicit named graph for all loads, queries and counts. The supervisor inventories the RocksDB files after each loader exits and before the next process opens the existing database. Reopening reproduced each profile's initial result. The check covers ordinary close and reopen on this fixture. All four profiles remain ineligible for the planned workload measurements. No timing was collected.

## Replay

Extract `packet.tar.gz` to a fresh directory. `PACKET_FILES.json` lists the SHA-256 digest of every archived file. Run these commands from within the extracted directory using standard-library Python:

```bash
python3 -B source/replay.py attempt-1 > replay.json
cmp replay.json REPLAY.json
python3 -B audit.py . > audit.json
cmp audit.json AUDIT.json
```

The raw responses, source, wheel manifest, installed-file identities and per-store inventories are in the packet. Database files themselves remain in the original run directory.
