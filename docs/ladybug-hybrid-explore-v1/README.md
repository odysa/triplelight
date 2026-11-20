# Ladybug hybrid-schema experiment: BSBM 10M Explore

The hybrid schema reduced the sum of query-to-full-JSON times by **4.41× and 4.39×** in two recorded passes over the same 20 frozen query instances. Every instance was faster in both observations. Gains are uneven: the four complex-OPTIONAL instances (q011, q012, q014, q015) improve only about 1.3×, while the largest reductions are around 20×. The tradeoff is a **58.9% larger database**, about **25.6% higher maximum query/export worker RSS**, and more import/preparation storage. Opening the hybrid database also took longer in this warm-cache experiment.

This is a comparison of two schemas **within Ladybug 0.22.3**, not a new TripleLight or cross-engine ranking. Do not substitute these times into historical benchmark tables or pool sessions. Two observations on a fixed workload do not establish statistical confidence or performance on other workloads.

## Recorded results

Primary time is from before query execution through complete bulk Arrow fetch and SPARQL JSON bytes ready in memory. Open, close, file writes/compression and correctness scoring are excluded. Each cell below sums 20 queries, with one fresh read-only process for each request.

| Recorded pass | Generic primary (s) | Hybrid primary (s) | Generic/hybrid ratio | Generic open sum (s) | Hybrid open sum (s) | Generic max worker RSS (GiB) | Hybrid max worker RSS (GiB) |
|---|---:|---:|---:|---:|---:|---:|---:|
| p1 | 19.923767 | 4.519276 | 4.409× | 0.482005 | 0.710071 | 1.347 | 1.692 |
| p2 | 19.436218 | 4.423862 | 4.393× | 0.481647 | 0.703089 | 1.348 | 1.693 |

Worker RSS is the Darwin process high-water mark, including imports, open, query, Arrow/JSON export, output storage and close. Reference decompression and bag/order scoring run separately after all measured workers exit. The high-water mark sampled at full JSON readiness is retained too. These are process RSS values, not pure engine heap sizes. The execution API/fetch split is descriptive because materialization can occur in either call; the composite is primary.

Each mapping emitted 247,885 uncompressed SPARQL JSON bytes per complete pass. Full component totals, individual observations, payload hashes and memory values are in [RESULTS.json](RESULTS.json) and the raw records.

## Workload, mapping and correctness

- BSBM Explore: 10,102,032 original unique RDF triples and 20 frozen query instances (20 distinct texts). This is Explore, not the separate BSBM BI workload.
- Generic: Resource and Literal nodes with per-predicate Resource/Literal relationship tables.
- Hybrid: Resource nodes retain literal STRUCT-list properties for ordinary patterns and simple OPTIONAL. Seven literal relationship tables and the full Literal node table remain for complex/nested OPTIONAL groups. The choice is structural and was frozen before import and performance measurement; no path was selected after observing timings.
- Original-triple conservation: 5,220,873 literal-column elements plus 4,881,159 resource edges equals 10,102,032. Retained duplicate storage is counted separately: 1,255,398 Literal nodes and 1,887,502 literal edges.
- The variable-predicate UNION enumerates literals from columns and only Resource-to-Resource edges from relation tables, avoiding duplicate answers from retained literal edges.
- Before scoring, both revised mappings passed 20 typed Jena bags and nine declared order checks. Fifteen tiny positive controls match explicit expected bags; three negative shapes are refused. The scored attempt passes all 120 typed bags and 54 order checks, including all warmup requests. Independent offline replay passes the same checks and re-sums primary totals.

Nested OPTIONAL is limited to child groups anchored by a variable first bound by the parent's mandatory triples. Literal struct/node identity crossings are refused. Explicit double(str()) ordering requires one mandatory audited literal predicate: every price edge and all 434,461 distinct price literal IDs were checked, covering 570,000 edges with zero invalid/non-finite values. The compiler refuses unaudited, invalid, non-finite or nullable cast domains. This is not a general implementation of SPARQL cast-error/null ordering. The fixture checks a numeric tie and refusal of an invalid-value domain.

## Storage and import costs

Sizes are logical bytes of complete database files, including retained duplicate literal storage. Import costs below are single observations with resource monitoring, not a controlled build-speed claim; caches were not reset. Conversion's peak RSS covers its Python worker, not its external sort child. Both database loads use a 10 GiB pool and four threads, and include validation/checkpoint/close.

| Cost | Generic | Hybrid |
|---|---:|---:|
| Database bytes | 1,606,807,552 (1.496 GiB) | 2,553,683,968 (2.378 GiB) |
| Additional column conversion | — | 69.359 s; peak RSS 2.019 GiB |
| Fresh database load | 153.450 s; peak RSS 1.770 GiB | 93.878 s; peak RSS 4.200 GiB |
| Common historical CSV-preparation directory, retained | 1,654,110,976bytes | same shared CSVs |
| Additional retained hybrid prepared outputs | — | 1,823,305,571bytes |
| Database + all retained import artifacts | 3,260,918,528bytes(3.037 GiB) | 6,031,100,515bytes(5.617 GiB) |

The shared historical CSV preparation cost was 413.431 s; it is attributed to both pipelines and was not rerun here. Hybrid conversion is additional. Retained import artifacts are reproducibility/workflow costs; they are distinct from the deployable database size. Allocated-byte counts are also recorded in RESULTS.json. Import monitoring used periodic directory walks; scored requests moved those walks outside the primary interval.

## Measurement contract

One untimed complete pass precedes two recorded passes. Each query's pair alternates which mapping runs first; pass 2 reverses query order and each pair's order. Every request uses a fresh read-only Python process, the same 10 GiB pool/four threads, Python 3.14.3, PyArrow 26.0.1 and Ladybug 0.22.3 on Darwin arm64 (Apple M1 Max, 64 GiB physical RAM; hardware observed after the run in HARDWARE.json). This is warm-cache: OS cache was not reset, and preparation/qualification also warmed files. File-page residency was not measured.

All 120 requests, including warmup, are required. A bad bag/order check, unsupported query, missing request, process/resource failure or pre/postflight identity drift would leave both mappings' totals null. No request was retried or replaced. Every process exited successfully, all guards passed, and identities were unchanged. All recorded response gates were computed after the final query/export worker exited.

The actual-file preflight verifies the source manifest, Literal.csv, audited price edge table, resource-ID map, both databases, references, package versions and all 40 deterministic query translations. The supervisor repeats identity/recompilation checks after collection. Query caps are 180 s/request and 1,800 s overall, with a 46 GiB owned-root stop threshold, 48 GiB free-space floor and 16 GiB worker RSS guard. The conservative interval from freeze creation to final completion was 474.466 s, below the overall cap.

## Earlier steps and failure disclosure

1. The historical generic relation mapping remains preserved. The scored generic compiler shares the explicit cast correction and domain guard with the hybrid compiler.
2. The pure-column prototype statically translated 15/20 queries; q004/q011/q012/q014/q015 were explicitly unsupported because of whole-group/nested OPTIONAL. Those 15 were syntax coverage, not full-dataset qualification or a performance result.
3. The first full hybrid qualification passed 20 bags but failed q023 order: ordering by the numeric-datatype cache field produced null keys for the custom USD datatype. Its response, gate and source are retained. The corrected translators cast lexical values only after the full domain audit. This correction and the nested-OPTIONAL guard preceded the single scored registration.
4. An initial tiny fixture failed baseline CSV import on quoted newlines; the retained successful tiny stages use serial CSV COPY for baseline import. The large corpus used its original loader. TRY_CAST capability probes failed and motivated the explicit finite-domain restriction.

## Per-query primary observations

Times below are milliseconds; both passes are shown without best-of selection.

| Instance | Generic p1 | Hybrid p1 | Generic p2 | Hybrid p2 |
|---|---:|---:|---:|---:|
| q001 | 156.438 | 80.852 | 154.050 | 80.418 |
| q002 | 1928.888 | 95.528 | 1920.233 | 95.926 |
| q003 | 1916.722 | 98.054 | 1919.820 | 95.890 |
| q004 | 594.609 | 105.492 | 554.181 | 102.929 |
| q005 | 1942.061 | 95.004 | 1924.603 | 95.532 |
| q006 | 2143.495 | 106.916 | 1924.235 | 99.081 |
| q007 | 753.516 | 119.293 | 685.950 | 115.640 |
| q008 | 1916.142 | 95.454 | 1917.704 | 95.681 |
| q009 | 1924.668 | 94.457 | 1918.537 | 96.105 |
| q010 | 377.239 | 113.635 | 367.913 | 113.434 |
| q011 | 910.801 | 709.566 | 904.066 | 685.611 |
| q012 | 924.428 | 698.052 | 910.819 | 689.127 |
| q013 | 369.279 | 110.619 | 366.437 | 105.125 |
| q014 | 955.128 | 754.648 | 887.407 | 712.783 |
| q015 | 903.758 | 688.889 | 879.274 | 687.776 |
| q016 | 652.111 | 93.092 | 665.661 | 94.317 |
| q019 | 668.243 | 93.532 | 652.117 | 92.824 |
| q022 | 321.757 | 117.347 | 318.902 | 116.252 |
| q023 | 319.946 | 117.246 | 321.457 | 115.848 |
| q024 | 244.535 | 131.598 | 242.851 | 133.563 |

## Evidence and replay

Download the complete [evidence packet](evidence.tar.gz) and verify it with [SHA256SUMS](SHA256SUMS). Extract the archive into an empty directory, then run the audit command below there. The database files and original CSV dataset are not redistributed.

Registration SHA-256: ff33a0b7adaf9a0e379d5ab52be4f1ef23d1e2e62d823f2f4f91afa958ccdd9b. The registration was announced before the first scored request. The packet contains the frozen schedule/source/protocol, all 120 raw responses/worker records/gates, Jena references, pre/postflight receipts, resource observations, import costs, controls and earlier failed stages. Database/CSV dataset files are excluded; their hashes and counts remain in the receipts. Re-executing the engine experiment requires those exact inputs and pinned packages.

Run the independent portable audit with Python's standard library, without an engine, network or executing archived benchmark drivers:

```sh
python3 audit_packet.py .
```

See [INDEPENDENT-AUDIT.json](INDEPENDENT-AUDIT.json) for 120 bag checks, 54 order checks and independently recomputed totals. This result supports schema tuning as a material factor in Ladybug performance, with explicit memory/storage costs. It establishes no new cross-engine ordering, cold-start benefit, broad product-performance claim or full SPARQL conformance.
