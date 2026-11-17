# LadybugDB comparison: first completed cohort

Host LUBM-100, native LadybugDB 0.22.3 with a generic lossless RDF mapping. All 14 queries match Jena on every repetition (one warm-up and five measured runs per system). This is a primary mapping result, **not a claim about Ladybug’s best native schema**. BSBM and a tuned-schema comparison are pending.

| System | Sum of query medians | Geometric mean | Answers |
|---|---:|---:|---|
| jena | 15525.74 ms | 61.624 ms | 14/14 match |
| ladybug | 19772.47 ms | 392.533 ms | 14/14 match |
| rdfdb-memory | 640.09 ms | 3.206 ms | 14/14 match |
| rdfdb-disk | 786.93 ms | 4.040 ms | 14/14 match |

## Per-query medians

| Query | Rows | Jena ms | Ladybug ms | rdfdb memory ms | rdfdb disk ms |
|---|---:|---:|---:|---:|---:|
| q1 | 4 | 8.976 | 123.543 | 0.847 | 0.678 |
| q2 | 264 | 2446.379 | 1154.636 | 6.220 | 5.786 |
| q3 | 6 | 5.768 | 305.681 | 0.621 | 0.601 |
| q4 | 10 | 7.902 | 145.715 | 0.573 | 0.818 |
| q5 | 532 | 13.457 | 266.113 | 1.044 | 1.517 |
| q6 | 795970 | 4896.477 | 4327.689 | 239.872 | 300.644 |
| q7 | 59 | 5.653 | 3817.072 | 0.673 | 0.767 |
| q8 | 5916 | 120.385 | 1863.168 | 8.389 | 13.021 |
| q9 | 4462 | 2713.050 | 3627.707 | 92.726 | 102.009 |
| q10 | 4 | 8.089 | 116.187 | 0.474 | 0.588 |
| q11 | 10 | 8.123 | 12.514 | 0.578 | 0.627 |
| q12 | 125 | 19.353 | 23.047 | 0.653 | 1.338 |
| q13 | 265 | 13.988 | 83.446 | 0.962 | 1.962 |
| q14 | 795970 | 5258.145 | 3905.956 | 286.462 | 356.579 |

## Data, versions and boundaries

- Input: 13,879,970 source statements; 13,409,395 distinct triples. Native loaded counts agree with the complete conversion manifest: 2,181,789 resource nodes, 1,121,935 literal nodes, 13,409,395 relationships. Each distinct triple is represented by one relationship, including rdf:type. Nodes are term-dictionary entries, not additional RDF statements.
- Input SHA-256: `49eb4a916134e35a582f159e26f5abef42e6c5fe379dc64c2e84069553c38e6d`; rehashed unchanged after the run.
- Hardware: Apple M1 Max host, 64 GB physical RAM. All systems run sequentially in the same host session. This is not a VM comparison and must not be mixed with the existing VM tables.
- rdfdb binary built from production source `cdba2e6`; SHA-256 `48d3f913a12ac4e9c922eb4aca446fdf957f1715d13a25bbada2f69f4a288fb7`. Native wheel library, Python, source and query hashes are in `lubm100-provenance.json`.
- Ladybug: 4 threads, 10,240 MiB buffer pool target. rdfdb: `RAYON_NUM_THREADS=4`, memory load uses `--parallel-load=4`; disk uses `--cache-mb=10240`. Jena 6.2.0: existing TDB2 store, `-Xmx10g`. These are cache/heap targets, not equal total-RSS limits; the memory backend holds its graph in RAM. Commands and per-process readiness times are in `results.json`.
- Fixed order: Jena, Ladybug, rdfdb memory, rdfdb disk. Warm results only; OS caches were not cleared. One session does not establish stable ratios or cold-read behavior.
- Timing boundary: full HTTP response, including result materialization, serialization and transfer. Client JSON parsing and full-answer hashing happen after the timer. Ladybug uses the native Cypher executor through a Python HTTP adapter; its Python iteration/JSON costs are included. This is not an isolated engine execution benchmark.
- The native schema stores resources/literals separately and creates per-predicate typed relationships. Constants use precompiled native integer primary keys. Queries are documented native translations of the original cohort, with no inference. The existing inference-free LUBM cohort contains duplicate pairs Q1/Q10 and Q6/Q14; totals retain that original weighting.
- Every repetition preserves and compares the complete projected RDF-term multiset, including duplicate rows and literal identity, against Jena. Canonicalization normalizes xsd:string/plain literals and language case. Source inputs, query text and the pinned rdfdb binary were retained. Agreement covers these queries, not every possible query or a proof of whole-store equivalence.

## Import cost

Conversion/deduplication took 285.63 s wall time (peak RSS 1,693,843,456 bytes). Native COPY, count verification and checkpoint took 40.29 s wall time (peak RSS 900,743,168 bytes). The end-to-end import is therefore about **325.92 s**, not 40.29 s. Conversion includes input hashing and externally sorted duplicate removal. The resulting Ladybug store occupied approximately 897 MiB (`du -sh`); this rounded figure is not a logical-byte measurement.

No same-session fresh index-build timings were collected for Jena or rdfdb disk in this cohort; their existing stores were reopened. Do not interpret startup/readiness as an index-build time.

## Limits and next comparison

The generic mapping exposes rdf:type as graph edges rather than native class tables. Large type hubs can produce very different planning costs from a tuned property-graph schema. The measurements do not establish that the remaining cost is solely the adapter or that this is Ladybug’s fastest plan. The class-table/typed-property comparison must be reported separately. BSBM’s OPTIONAL, UNION, language/date filters, ordering and result bags need complete-answer checks before performance claims.

Reproduction scripts are in `bench/ladybug/`. All raw samples, compiled Cypher, logs, count manifests and provenance are in this directory. The initial native-script hash used for conversion and the measured adapter is retained; later additional UNION fixtures were validated separately and did not change the running LUBM path.
