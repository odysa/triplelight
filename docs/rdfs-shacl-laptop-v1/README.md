# Embedded RDFS and SHACL on the laptop

This run compares TripleLight and Apache Jena 6.2.0 on the same LUBM-1 and LUBM-10 data. The RDFS workload contains the 14 original queries and two inference count probes. The SHACL workload checks contact fields and then deliberately requires an IRI where the data supplies a string.

## Results

All 16 typed query answers matched for both sizes in simple and RDFS modes. The RDFS answers also matched the separate streaming closure. RDFS inferred 8,330 and 106,409 people and 3,494 and 44,518 degree relationships on the two inputs. Both counts were zero in simple mode. The conforming shapes yielded no violations. The deliberately failing shapes yielded 8,330 and 106,409 exactly matching report entries.

| Inferred LUBM queries | TripleLight ms | Jena ms | TripleLight / Jena |
| --- | ---: | ---: | ---: |
| LUBM-1 | 12.42 | 45.95 | 0.27 |
| LUBM-10 | 131.94 | 234.06 | 0.56 |

The table sums the warm medians of the 14 original queries and excludes loading, Jena view setup, and the two count probes. On LUBM-10, TripleLight was faster on eight of the 14 queries. It took 23.4 ms versus 1.0 ms on LUBM Q5 and 22.9 ms versus 0.33 ms on Q13, which returned no rows. RDFS process peak RSS on LUBM-10 was 1,029 MiB for TripleLight and 1,928 MiB for Jena.

| SHACL validation only | TripleLight ms | Jena ms | TripleLight / Jena |
| --- | ---: | ---: | ---: |
| LUBM-1 conforming | 164.69 | 8.08 | 20.38 |
| LUBM-1 deliberate violations | 159.69 | 32.62 | 4.90 |
| LUBM-10 conforming | 2,870.98 | 94.39 | 30.42 |
| LUBM-10 deliberate violations | 2,872.26 | 709.33 | 4.05 |

Jena kept its data indexes across validation calls. TripleLight built them anew on each call. On LUBM-10, TripleLight spent about 0.92 s loading data into a triple vector, while Jena spent about 2.3 s loading its indexed graph. For the 106,409-entry report, encoding took 274 ms in TripleLight and 675 ms in Jena. Full-process peak RSS was 2,301 versus 1,151 MiB for conforming data and 3,252 versus 4,000 MiB for the large report.

[Per-query and phase timings](timings.md) report the two paired ratios and all measured phases.

## Sources and inputs

- TripleLight combines [RDFS at 3d877d67](https://github.com/odysa/triplelight/commit/3d877d67015ebc6b0e7a8af0baa52e2a19de831e) and [SHACL Core at f63f8ac0](https://github.com/odysa/triplelight/commit/f63f8ac0f484f36437a7d6c7362f0789fdb572d7) in local benchmark commit `f55414ebcefe9645c927f2f47bcc0d4af262fd3d`. These are the candidate revisions qualified before their full merge gates. The RDFS arm uses the default graph.
- UBA 1.7 seed 0, University0 for LUBM-1 and all ten generated universities for LUBM-10. Jena RIOT converted the shared RDF/XML sources and the [LUBM university ontology](https://swat.cse.lehigh.edu/onto/univ-bench.owl) into one explicit N-Triples input per dataset. There were 103,410 and 1,317,006 input lines. Both engines read the same files. See [artifact digests](digests.json).
- Both workers used native macOS ARM64 on the same 64 GB Apple Silicon host. The TripleLight release build used Rust 1.100.0 and its lockfile. The Jena worker used OpenJDK 21.0.11 and a fixed 8 GiB Java heap ceiling. Rayon used four workers. The JVM saw four active processors. Peak resident set size includes the whole worker process.

## Measurement

Each dataset passed an untimed typed comparison in both modes and with both shapes graphs before its timed runs began. Each timed workload ran four fresh processes in TripleLight, Jena, Jena, TripleLight order. A process first loads the input, then repeats each operation seven times. The first two repetitions are warmups. Each process preserves the complete answer from its first repetition and checks the cardinality throughout. The tables show the median of the two process medians per engine, and the observed range of the two neighboring TripleLight-to-Jena ratios. This is a descriptive laptop run with the operating-system cache in its normal state.

RDFS compares simple and RDFS graph matching separately. Both measure query parsing and materialization of SELECT answers, with SPARQL JSON encoding outside the measured call. TripleLight uses a memory snapshot and the embedded query executor. Jena uses a memory graph and wraps it once with `RDFSFactory.graphRDFS`. The tables separate Jena's view construction from its query calls. TripleLight prepares the RDFS view inside each query call. A separate Jena RIOT streaming closure is queried under simple entailment to verify the inferred answers. The graph views share the six RDFS rules covered by the earlier [LUBM-1 reference](../rdfs-jena-lubm1-v1/README.md).

SHACL measures loading the data, loading the shapes RDF graph, running the public validation API to build a full report, and encoding that report as N-Triples in memory. TripleLight loads triples into a vector and indexes them inside each validation call. Jena loads an indexed graph before validation. Both parse shapes as part of the validation call. Report encoding includes forming RDF triples where the report API requires it. File writes are outside the encoding timer. Peak process RSS includes loading and report encoding. The trade-off between Jena's up-front graph load and its validation time is part of these measurements.

The answer gates compare unordered typed SELECT multisets, including lexical forms and unbound terms. SHACL compares conformance and the typed multiset of focus node, result path, value, source shape, constraint component and severity. Generated report blank-node identifiers and optional message text are outside that comparison. Full raw answers and reports were retained with the run. The committed [timings](raw/) and [structured summaries](results.json) preserve all measured-process records.
