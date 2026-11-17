# Embedded RDF query comparison with LadybugDB

On these prepared databases and RDF mappings, rdfdb has lower total fully consumed query time than the tested Ladybug configurations. Both result APIs pass the corrected result checks. This is an application-and-adapter comparison, not an engine-only or general SOTA claim.

Fourteen LUBM-100 queries and twenty BSBM-10M queries are measured through Python APIs, in process on both sides. Every result row is materialized and decoded into common RDF terms. No HTTP server or response transfer is in the measured boundary.

## Results

Values below are sums of per-query medians, not a single query latency or throughput. Each median pools six measured samples across two fresh processes in reversed system order; each process also has one excluded first-use sample per query. Geometric means weight the query templates equally. RSS includes hashing and the whole process, not just execution.

### lubm100

| Configuration | Sum of query medians (ms) | Geomean (ms) | Process RSS range (GiB) |
|---|---:|---:|---:|
| rdfdb owned Python objects | 2834.955 | 2.149 | 1.49–1.64 |
| rdfdb JSON, parsed and consumed | 2852.698 | 1.949 | 1.71–1.78 |
| Ladybug generic RDF mapping | 14757.027 | 270.070 | 2.82–2.88 |
| Ladybug LUBM-tuned mapping | 4483.671 | 10.264 | 1.17–1.24 |

### bsbm10

| Configuration | Sum of query medians (ms) | Geomean (ms) | Process RSS range (GiB) |
|---|---:|---:|---:|
| rdfdb owned Python objects | 27.199 | 0.583 | 0.33–0.33 |
| rdfdb JSON, parsed and consumed | 33.275 | 0.734 | 0.33–0.33 |
| Ladybug generic RDF mapping | 4008.680 | 55.218 | 1.76–1.77 |

The LUBM tuned mapping uses explicit RDF type-signature node tables and multi-valued properties. It is a stronger native setup than the generic Resource/Literal graph, and benefits from the dataset structure. BSBM uses only the generic mapping; no tuned BSBM result is claimed.

### Large outputs dominate the LUBM application boundary

Q6 and Q14 each return 795,970 rows. Excluding those two queries changes the totals substantially:

| Configuration | Sum excluding Q6/Q14 (ms) | Q6/Q14 share of full sum |
|---|---:|---:|
| Ladybug generic RDF mapping | 9624.720 | 34.8% |
| Ladybug LUBM-tuned mapping | 154.238 | 96.6% |
| rdfdb JSON, parsed and consumed | 112.773 | 96.0% |
| rdfdb owned Python objects | 85.592 | 97.0% |

The common RDF decoding is real consumer work for this workload, but its cost depends on the mapping. Ladybug stores serialized RDF-term JSON strings, which this adapter parses; a different columnar RDF representation could reduce that overhead. These totals must not be described as native engine execution times.

For Q14, rdfdb object API return is 857.7 ms and JSON API return is 209.7 ms, but complete consumption is 1480.8 and 1371.8 ms respectively. The apparent return-time advantage largely shrinks once the JSON consumer parses and decodes all terms. The two whole-cohort RDF totals differ by less than 1%, which is not a robust winner.

Q9 is also sensitive to process variation: owned rdfdb medians are 68.5 / 44.6 ms across the two processes, against tuned Ladybug 61.2 / 74.3 ms. The winner reverses. Do not infer a stable Q9 advantage from the pooled values.

## Per-query fully consumed medians

### lubm100

| Query | Rows | rdfdb owned Python objects (ms) | rdfdb JSON, parsed and consumed (ms) | Ladybug generic RDF mapping (ms) | Ladybug LUBM-tuned mapping (ms) |
|---|---:|---:|---:|---:|---:|
| q1 | 4 | 0.099 | 0.059 | 108.605 | 1.709 |
| q10 | 4 | 0.073 | 0.049 | 109.928 | 1.576 |
| q11 | 10 | 0.081 | 0.057 | 11.178 | 1.398 |
| q12 | 125 | 0.809 | 0.607 | 6.125 | 2.120 |
| q13 | 265 | 1.313 | 0.944 | 76.064 | 3.289 |
| q14 | 795970 | 1480.828 | 1371.752 | 2505.681 | 2102.306 |
| q2 | 264 | 5.261 | 4.975 | 845.515 | 20.373 |
| q3 | 6 | 0.092 | 0.068 | 287.777 | 1.620 |
| q4 | 10 | 0.208 | 0.199 | 29.964 | 1.742 |
| q5 | 532 | 1.259 | 1.379 | 236.147 | 3.274 |
| q6 | 795970 | 1268.535 | 1368.174 | 2626.626 | 2227.127 |
| q7 | 59 | 0.268 | 0.324 | 3179.505 | 1.921 |
| q8 | 5916 | 20.360 | 28.141 | 1530.399 | 48.616 |
| q9 | 4462 | 55.768 | 75.971 | 3203.512 | 66.599 |

### bsbm10

| Query | Rows | rdfdb owned Python objects (ms) | rdfdb JSON, parsed and consumed (ms) | Ladybug generic RDF mapping (ms) |
|---|---:|---:|---:|---:|
| q001 | 3 | 0.217 | 0.213 | 7.134 |
| q002 | 16 | 0.738 | 1.022 | 18.029 |
| q003 | 13 | 0.636 | 0.823 | 19.676 |
| q004 | 1 | 0.366 | 0.424 | 272.952 |
| q005 | 20 | 0.828 | 1.118 | 27.039 |
| q006 | 22 | 1.018 | 1.265 | 28.059 |
| q007 | 2 | 0.422 | 0.514 | 92.476 |
| q008 | 16 | 0.718 | 0.977 | 27.580 |
| q009 | 19 | 0.767 | 1.036 | 26.279 |
| q010 | 5 | 10.063 | 12.510 | 40.174 |
| q011 | 3 | 0.351 | 0.465 | 590.877 |
| q012 | 10 | 0.606 | 0.891 | 553.402 |
| q013 | 5 | 8.113 | 8.955 | 33.458 |
| q014 | 3 | 0.342 | 0.386 | 562.473 |
| q015 | 8 | 0.457 | 0.663 | 537.820 |
| q016 | 4 | 0.364 | 0.487 | 596.010 |
| q019 | 6 | 0.607 | 0.873 | 564.601 |
| q022 | 1 | 0.258 | 0.290 | 3.734 |
| q023 | 3 | 0.256 | 0.270 | 3.756 |
| q024 | 10 | 0.072 | 0.093 | 3.151 |

## Startup, versions and resources

- Host: Apple M1 Max, 64 GiB RAM; Python 3.14.3, Ladybug 0.22.3, PyArrow 26.0.1. Four query threads; nominal cache targets 16 GiB. rdfdb uses its default per-snapshot aggregate block-cache target; Ladybug uses a buffer-pool target. Neither is total RSS or a hard memory bound.
- LUBM uses the frozen `fe6db89` Python binding; its extension SHA-256 is `d07afded246f314e5302be0b141ca90189c7c6c5df156281fae06b3c3d198477`. BSBM uses fixed main `5b3970b379d307d071ca05d3a1d1cecb2d7094b2`, extension SHA-256 `898940920addbc19743b4b06714787d0766415e55ffa34790b91da1c61de05e1`. These are separate matched sessions, not one binary across both datasets.
- All fourteen LUBM source queries contain neither ORDER BY nor XSD constructor casts. The cast fix `c58383f` therefore does not change the query forms exercised by that cohort. This is a source-scope check, not a rerun of the LUBM timings on the later binary.
- Existing native stores were reused from the previous mapping experiment; rdfdb uses dedicated durable stores created for this task. LUBM has 13,409,395 unique triples and BSBM 10,102,032. Source NT hashes match the earlier validated inputs after both sessions.
- No updates or compaction occur during query measurements. rdfdb statistics are awaited on an unchanged base. Native queries use parameterized execute with the driver prepared-plan cache, not an answer cache.
- OS caches are not cleared. Only two process orders were tested on a shared host under the project benchmark lock. There are no confidence intervals; small differences and single-query winners are not robust conclusions.

Open and statistics-ready costs are distinct. Queries are available before statistics finish, but the timings below wait for readiness. The first query sample is therefore first **post-ready use**, not immediate-query-from-open latency.

| Dataset / configuration | Open ms, process 1 / 2 | Ready ms, process 1 / 2 |
|---|---:|---:|
| lubm100 / Ladybug generic RDF mapping | 127.0 / 38.7 | 127.0 / 38.7 |
| lubm100 / Ladybug LUBM-tuned mapping | 237.9 / 56.9 | 237.9 / 56.9 |
| lubm100 / rdfdb JSON, parsed and consumed | 157.1 / 153.5 | 4278.5 / 4173.9 |
| lubm100 / rdfdb owned Python objects | 133.6 / 142.3 | 4263.6 / 4179.4 |
| bsbm10 / Ladybug generic RDF mapping | 119.8 / 31.5 | 119.8 / 31.5 |
| bsbm10 / rdfdb JSON, parsed and consumed | 108.3 / 125.5 | 3480.8 / 3479.7 |
| bsbm10 / rdfdb owned Python objects | 116.5 / 121.3 | 3519.6 / 3110.5 |

Import is excluded from query timings. The new rdfdb stores took 50.7 s and 81.8 s to open/import, with statistics ready at 55.4 s and 85.7 s. Import uses the API’s default worker count. Native conversion and loading were not repeated in these sessions, so no matched import-speed claim is made.

## Correctness finding and recovery

The first BSBM run was aborted, not included in the totals. With `ORDER BY xsd:double(str(?price))`, rdfdb q023 returned prices `7851.30, 2216.71, 5947.37`; Jena returned `2216.71, 5947.37, 7851.30`. The result bag was identical. The expression evaluator lacked constructor casts, so every sort key was an error and storage order survived. The core owner fixed this on `c58383f`; the fixed binding is used for the entire new BSBM cohort. This shows why bag equality alone is insufficient.

Two native-adapter issues were also corrected before the new BSBM cohort: generated sort helper columns are excluded from the public projection, and the cast sorts lexical text via `CAST(...value AS DOUBLE)` rather than a numeric-only projection that is null for custom USD literals. The corrected actual database fixture checks the numeric order 1, 2, 10, not lexical order or all-null ties. Prior bags-only measurements did not establish ordering correctness.

Final evidence covers 448 complete LUBM responses and 480 complete BSBM responses, including warmups. Every complete result bag matches independent Jena CLI exports, preserving variables, duplicates, term kind, lexical text, datatype, language and unbound values. BSBM also passes 216 ordered-response checks (nine ordered queries × three configurations × eight responses), allowing arbitrary order within tied keys. Comparators are scoped to these homogeneous lexical, finite double-cast and timezone-free datetime keys, not all SPARQL order semantics.

Five pure standard-library semantic tests and four actual cross-engine fixtures pass. Fixtures cover distinct integer lexical forms/datatypes, language tags, duplicates, empty strings versus unbound values, and the custom-typed cast/order case across rdfdb objects, rdfdb JSON and Ladybug Arrow. The initial missing-predicate fixture error and the aborted BSBM cohort are retained. These checks do not certify full SPARQL compliance or arbitrary blank-node equivalence.

## Evidence and reproduction

- [Measurement contract and commands](../../bench/embedded/README.md); [machine-readable summary](summary.json).
- `raw/lubm100/` and `raw/bsbm10/` contain per-sample clocks, signatures, commands, process logs, reference signatures and measured source snapshots. `raw/invalid-bsbm-before-fix/` is deliberately excluded.
- `raw/q023-order-failure.json` retains actual failed and reference rows. `raw/*binding-provenance.json` and postcheck files pin the binaries; no editable source was used during a timed pass.
- Independent full reference JSON is gzip-retained at `/private/tmp/rdf-jianlin-embedded22/{lubm100,bsbm10}-reference/`; the report includes signatures and export commands, not those large payloads. Dataset files and database stores remain local and are not embedded in this report.
- `bench/embedded/summarize.py` verifies successful process exits and complete reference matches before aggregating. An explicit dataset selector is needed to summarize LUBM while the old invalid BSBM record is present.

Next optimization work should keep the application boundary explicit: reduce large-result Python/RDF materialization costs or introduce a separately measured columnar result interface. The current APIs materialize results; no streaming, zero-copy, hard memory cap or universal performance guarantee is claimed.
