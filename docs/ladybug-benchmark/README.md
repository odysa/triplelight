# rdfdb versus LadybugDB: measured host comparison

**The tuned-schema comparison is much closer than the initial generic-mapping result.** On LUBM-100, summed warm medians were **567 ms for rdfdb memory, 735 ms for rdfdb disk and 820 ms for tuned LadybugDB**. The observed disk margin is about 12%; one shared-host session does not establish a robust winner at that margin. On BSBM-10M, the tested generic Ladybug mapping totaled **4,632 ms**, versus **23.2 / 48.0 ms** for rdfdb memory/disk. A tuned BSBM schema was not measured.

Every measured query returned the same complete projected result bag, including duplicates, as Jena: 14 LUBM queries and 20 BSBM queries. The primary final tables use Ladybug’s native **parameterized plan reuse and bulk Arrow output**. There is no answer cache.

## LUBM-100: same-session schema comparison

| System | Sum of warm medians | Geometric mean | Matching queries |
|---|---:|---:|---:|
| jena | 15953.633 ms | 60.864 ms | 14/14 |
| ladybug-generic | 11866.348 ms | 261.692 ms | 14/14 |
| ladybug-tuned | 820.125 ms | 10.383 ms | 14/14 |
| rdfdb-memory | 567.321 ms | 3.105 ms | 14/14 |
| rdfdb-disk | 734.895 ms | 3.781 ms | 14/14 |

Both Ladybug schemas use the same native API and output path in this run. The tuned schema reduces the generic mapping’s total from 11,866 ms to 820 ms. This comparison combines the effects of the documented physical schema and its equivalent query translations; it does not prove optimal native plans or isolate an individual optimization.

Tuned Ladybug is also lower on Q9 than rdfdb disk: **47.390 versus 100.588 ms**. The aggregate result must not be presented as a win on every query.

## BSBM-10M: generic RDF mapping

| System | Sum of warm medians | Geometric mean | Matching queries |
|---|---:|---:|---:|
| jena | 4136.528 ms | 17.684 ms | 20/20 |
| ladybug | 4632.139 ms | 76.680 ms | 20/20 |
| rdfdb-memory | 23.211 ms | 0.894 ms | 20/20 |
| rdfdb-disk | 47.976 ms | 1.509 ms | 20/20 |

All 20 captured SELECT queries are included, with OPTIONAL, both UNION shapes, numeric/date/language filters, DISTINCT, ordering, limits and variable predicates translated. DESCRIBE/CONSTRUCT are excluded by the pre-existing captured SELECT cohort, not selected after observing performance. No tuned property-graph BSBM schema or 100M+ Ladybug run is claimed.

## Earlier configurations and native compilation

The first LUBM run used row-by-row Python conversion and compile-per-request Cypher: Ladybug totaled 19,772 ms. It remains archived in `lubm100-generic/` and is not the best-schema headline. BSBM with bulk output but compile-per-request Cypher totaled 33903.715 ms; the final parameterized-plan run totaled 4632.139 ms. These are separately recorded cohorts, not an isolated causal A/B.

For example, default BSBM q002 reported roughly 3.83 s native compilation and 21 ms execution, with 3.86 s HTTP latency. Parameterized q002 had about 33 ms HTTP latency. Its native compilation getter still reported the prepared plan’s original compilation time (about 3.69 s). **Do not add that reused metadata to per-request latency or interpret it as repeated work.** Parameterized plans can also differ from literal plans. Raw getters are retained as supplementary telemetry.

## Setup and timing boundary

- Apple M1 Max host, 64 GB physical RAM; native host processes, not the earlier Linux VM comparison. LadybugDB 0.22.3, Python 3.14.3, PyArrow 26.0.1 and Jena/Fuseki 6.2.0.
- rdfdb immutable baseline: production source `cdba2e6`, binary SHA-256 `48d3f913a12ac4e9c922eb4aca446fdf957f1715d13a25bbada2f69f4a288fb7`. Later repository rebases do not change the measured binary.
- Requested settings: Ladybug 4 native threads and 10,240 MiB buffer pool; rdfdb `RAYON_NUM_THREADS=4`, memory load `--parallel-load=4`, disk `--cache-mb=10240`; Jena `-Xmx10g`. Cache/heap targets are not equal total-RSS limits. rdfdb memory retains its graph in RAM.
- Sequential servers in one exclusive team benchmark window: Jena, Ladybug generic, optional Ladybug tuned, rdfdb memory, rdfdb disk. One warm-up plus five measured requests per query. OS caches were not cleared; fixed order and shared-host variation limit generalization.
- The timer covers the complete HTTP response, including backend execution, materialization, serialization and transfer. Client JSON decoding, supplementary-header parsing and canonical hashing occur after the timer. Ladybug uses a Python HTTP bridge and its native Cypher executor; this is not an engine-kernel-only comparison.
- Native Arrow output assembles complete SPARQL JSON from pre-encoded RDF-term properties. RDF term encodings and URI-to-surrogate-ID mappings are generated during import/query translation, outside query timing. Auxiliary mapping files are separate from native engine-store size. These are explicit choices of this adapter.
- The parameterized native API retains prepared plans; no query answers are retained. rdfdb uses its public HTTP endpoint without an added adapter plan cache. First-use planning remains in warm-up/startup costs. Commands, readiness times, raw warm-ups and all measured repetitions are saved.
- The result check sorts complete projected rows and hashes them with SHA-256, preserving multiplicities and normalizing xsd:string/plain literals and language case. For ORDER BY queries, selected result bags are checked; row sequence is not independently asserted. Full response bodies are not archived, so recomputing answer hashes requires replaying the saved scripts with the inputs.
- Both original N-Triples files and the immutable rdfdb binary were rehashed unchanged at the end. Final-run source hashes also match the frozen source snapshot. Query agreement is scoped evidence, not proof of whole-store equivalence for arbitrary queries.

## Mapping and conservation checks

| Dataset | Source statements | Distinct triples | Resource nodes | Literal nodes (generic) |
|---|---:|---:|---:|---:|
| LUBM-100 | 13,879,970 | 13,409,395 | 2,181,789 | 1,121,935 |
| BSBM-10M | 10,102,032 | 10,102,032 | 1,489,462 | 1,255,398 |

The generic mapping stores each distinct RDF triple as one typed relationship, including rdf:type, with separate resource/literal node tables and per-predicate relationships. Resource/literal nodes are dictionary entries, not additional RDF facts. Literal lexical forms, datatype and language remain distinguishable even when numeric values are projected for filters.

The tuned LUBM mapping uses one node table for each exact set of explicit RDF types, preserving multiply typed resources, and list columns preserving multi-valued literal properties. Resource relationships are partitioned by predicate and endpoint type signatures. The conservation equation is **2,310,694 explicit type facts + 4,450,766 literal values + 6,647,935 resource relationships = 13,409,395 RDF triples**. All native loaded counts agree. No subclass inference is added.

The existing inference-free LUBM cohort uses concrete classes as documented in the repository; Q1/Q10 and Q6/Q14 are duplicate pairs after that adaptation. Totals retain its original weighting. BSBM uses the existing captured SELECT mix. Source/query hashes and all compiled Cypher are retained.

## Import measurements

| Mapping | Python conversion/preparation | Native COPY/checkpoint | Observed workflow import | Peak RSS: conversion / native load |
|---|---:|---:|---|
| LUBM generic | 285.63 s | 40.29 s | 325.92 s | 1,693,843,456 / 900,743,168 bytes |
| BSBM generic | 413.52 s | 72.65 s | 486.17 s | 1,846,984,704 / 2,521,628,672 bytes |
| LUBM tuned | 47.16 s after shared conversion | 19.26 s | 352.05 s including shared 285.63 s conversion | 388,808,704 / 1,180,975,104 bytes |

Tuned preparation reuses generic CSV artifacts, but does not require loading the generic Ladybug database. The workflow total therefore includes shared conversion plus tuned transformation/COPY, not the unrelated generic COPY. This is an observed Python-adapter workflow, not a minimum possible Ladybug ingestion time. Existing Jena/rdfdb disk stores were reopened; no fresh paired index-build timings were collected for them in these cohorts. Readiness is not an index-build measurement.

Typed node lists are transferred through Parquet. A failed CSV-list fixture doubled three logical literal properties to six elements by splitting serialized JSON at commas; the conservation check caught it before timing. Failing logs, the fix and passing multi-type/multi-value/UNION/date/Arrow/parameterized fixtures are retained in `validation/`.

## Reproduction and evidence

Install `ladybug==0.22.3` and `pyarrow==26.0.1` in a Python environment. Acquire the shared benchmark lock before any import, service or timing. [Source scripts](source/native.py) include the generic mapper, [tuned mapper](source/tuned.py), [native UNION translation](source/union_queries.py), [bulk JSON encoder](source/arrow_output.py), [parameter binding](source/parameters.py), [semantic fixtures](source/test_native.py) and [serial driver](source/compare.py).

```sh
python native.py prepare INPUT.nt CSV_DIR
python native.py load CSV_DIR DB_PATH --memory-mb 10240 --threads 4
python native.py compile CSV_DIR QUERY_DIR COMPILED.json
python tuned.py prepare LUBM_CSV_DIR TUNED_DIR
python tuned.py load TUNED_DIR TUNED_DB --memory-mb 10240 --threads 4
python tuned.py compile TUNED_DIR LUBM_QUERY_DIR TUNED_COMPILED.json
# compare.py --help lists explicit input/binary/query/output paths.
# Use --ladybug-output arrow --ladybug-plan-mode parameterized.
# Add --tuned-database and --tuned-compiled for the paired LUBM run.
```

- [Final LUBM samples and commands](lubm100-bulk-paired/results.json)
- [Final BSBM samples and commands](bsbm10-parameterized/results.json)
- [BSBM compile-per-request samples](bsbm10-compare/results.json)
- [Tuned mapping manifest](imports/lubm100-tuned-csv-manifest.json)
- [BSBM mapping manifest](imports/bsbm10-csv-manifest.json)
- [Post-run input/binary/source checks](imports/final-after.json)
- [Native engine-file and auxiliary-map sizes](imports/footprint.json)
- Frozen compile-mode, parameterized and final service source snapshots are retained in their named directories. Native library/source/query hashes for the initial cohort remain in `lubm100-generic/`; the later service provenance is in `imports/`.

Input SHA-256 values: LUBM `49eb4a916134e35a582f159e26f5abef42e6c5fe379dc64c2e84069553c38e6d`; BSBM `3a207c6725820036eec87d08dd414090d3548ac2839ca192414dad6e352f680e`. Generated inputs and native stores are retained on the benchmark host, not bundled with this small report.

Project sources: [LadybugDB](https://github.com/LadybugDB/ladybug), [0.22.3 release](https://github.com/LadybugDB/ladybug/releases/tag/v0.22.3). The separate RDFLib proof-of-concept adapter is not the measured execution path.

## Complete per-query medians

### LUBM-100, paired cached-plan/bulk cohort

| Query | Rows | jena | ladybug-generic | ladybug-tuned | rdfdb-memory | rdfdb-disk |
|---|---:|---:|---:|---:|---:|---:|
| q1 | 4 | 8.713 | 125.644 | 2.723 | 0.898 | 0.453 |
| q2 | 264 | 2430.664 | 1029.347 | 21.955 | 5.879 | 5.875 |
| q3 | 6 | 5.653 | 338.315 | 3.608 | 0.695 | 0.598 |
| q4 | 10 | 5.925 | 37.003 | 4.827 | 0.595 | 0.687 |
| q5 | 532 | 11.522 | 256.881 | 4.326 | 1.163 | 1.471 |
| q6 | 795970 | 4964.100 | 879.711 | 368.729 | 231.222 | 303.289 |
| q7 | 59 | 5.534 | 3470.781 | 4.197 | 0.516 | 1.105 |
| q8 | 5916 | 125.258 | 1641.556 | 22.403 | 7.090 | 13.626 |
| q9 | 4462 | 2904.512 | 2974.381 | 47.390 | 73.951 | 100.588 |
| q10 | 4 | 8.396 | 120.149 | 2.638 | 0.470 | 0.570 |
| q11 | 10 | 8.591 | 14.744 | 2.474 | 0.618 | 0.572 |
| q12 | 125 | 19.415 | 8.441 | 2.940 | 0.726 | 1.204 |
| q13 | 265 | 15.379 | 86.280 | 3.629 | 0.956 | 1.386 |
| q14 | 795970 | 5439.970 | 883.114 | 328.286 | 242.543 | 303.471 |

Times in milliseconds; lower medians in one session are descriptive, not confidence intervals.

### BSBM-10M, cached-plan/bulk cohort

| Query | Rows | jena | ladybug | rdfdb-memory | rdfdb-disk |
|---|---:|---:|---:|---:|---:|
| q001 | 3 | 19.325 | 11.464 | 0.861 | 0.883 |
| q002 | 16 | 24.112 | 33.344 | 0.765 | 1.669 |
| q003 | 13 | 16.089 | 36.623 | 0.714 | 1.347 |
| q004 | 1 | 11.216 | 294.498 | 0.623 | 1.011 |
| q005 | 20 | 17.462 | 37.433 | 0.814 | 1.694 |
| q006 | 22 | 16.470 | 36.799 | 0.907 | 1.855 |
| q007 | 2 | 14.203 | 115.048 | 0.750 | 1.077 |
| q008 | 16 | 13.868 | 42.803 | 0.840 | 1.586 |
| q009 | 19 | 13.631 | 40.742 | 0.766 | 1.517 |
| q010 | 5 | 1700.277 | 42.911 | 5.276 | 14.270 |
| q011 | 3 | 6.485 | 630.969 | 0.574 | 0.995 |
| q012 | 10 | 8.273 | 636.499 | 0.858 | 1.268 |
| q013 | 5 | 2227.305 | 43.346 | 4.474 | 11.503 |
| q014 | 3 | 6.473 | 647.587 | 0.680 | 1.128 |
| q015 | 8 | 6.943 | 629.272 | 0.788 | 1.164 |
| q016 | 4 | 6.993 | 674.203 | 0.734 | 1.065 |
| q019 | 6 | 7.249 | 658.058 | 0.873 | 1.429 |
| q022 | 1 | 6.624 | 6.817 | 0.596 | 0.896 |
| q023 | 3 | 7.663 | 6.769 | 0.595 | 0.999 |
| q024 | 10 | 5.868 | 6.956 | 0.723 | 0.617 |

Times in milliseconds; lower medians in one session are descriptive, not confidence intervals.
