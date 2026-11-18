# Independent BSBM 100M BI reference

All **60 reference responses** and eight q7 cutoff diagnostics are available. Of the 60, **59 are original full-query Jena responses and one is explicitly labelled Jena group-key partitioned execution** (`m002_p00_q1`). No measured competitor or rdfdb output was used to construct them. Every assembled reference passed the existing order, cutoff and frozen-input checks. This is a correctness-reference report, not a comparative performance result.

## Original attempts and failures

The first Jena run attempted all 60 requests with a 180-second timeout: 57 passed; `m002_p00_q1`, `m003_p00_q1` and `m003_p01_q3` timed out. A separate 600-second retry recovered both mix-3 responses. The mix-2 q1 still timed out. A planning experiment disabled BGP reordering on the unchanged q1 text; mix-2 again timed out at 300 seconds. One overlap control matched exactly, and the remaining controls were explicitly stopped once that negative result was established. A prior configuration startup failed before any query because of a generated Turtle quoting error. All these attempts, timeouts and the early-stop receipt are retained. They were not converted to passing original-query runs.

## Partitioned q1 reference

The remaining q1 uses an independently reviewed, Jena-only execution protocol:

1. Enumerate the entire domain satisfying `?productType a bsbm:ProductType`: **2,011 unique IRIs**.
2. Restrict the original grouped query to disjoint VALUES sets of at most eight group keys. This produces at most eight groups per request, so the original LIMIT 10 cannot truncate a partition. Preserve each group's native Jena COUNT; do not sum counts across different product types.
3. Merge every group, apply the original descending-count / ascending-productType order, and take LIMIT 10.

Each GROUP BY key belongs to exactly one partition, so every matching row contributes to precisely the same group as in the full query. Multi-typed products contribute independently to each type, as the original query requires. Empty groups remain absent. The procedure preserves the exact positive integer counts and URI identities.

Three unit tests cover partition/IRI restrictions, domain coverage, duplicate groups and global cutoff behavior. An independent 90-triple Jena fixture includes 15 type keys, multi-typed products, multiple producer/reviewer join paths, an empty group and more than ten tied groups. Partitions of 1, 7 and 8 keys exactly matched its full-query bag and sequence.

At 100M scale all four q1 instances were evaluated in 252 partitions each (**1,008 partition requests**). All three available full-query q1 controls matched exactly, including sequence. A separate postcheck regenerated every partition query from its original source and domain, reassembled every result and checked coverage. The missing mix-2 instance uses this partitioned result. Its original full query remains a timeout; no execution-speed claim follows from the partition procedure.

## Frozen policies and provenance

The workload is bound to NT SHA-256 `6e4ef939f05b15677802d17257096de9da32a452fe33dd40c07d0b0537ee430d` and 98,861,444 unique triples. The separately frozen exact inputs are `63e127b60ac8de510a172748108639f0834e17bc3772e1d16ce226404aa58cca`. Whole-result rules were preregistered before the first 100M BI output; after collection, the final value policy binds all 68 reference files and retains those rules unchanged:

- Value-policy SHA-256: `7b3f640bbab516db80ceba87549b9670802a93c5ff77181bdda66c0855fc3dfb`.
- COUNT-policy SHA-256: `5049fb75210fc6fc78ebf1fb2553b88baef197ae73495b70f110a203a70a4663`.

Strict, COUNT-compatible and value-compatible scores remain distinct. Value-compatible checking includes the registered rounding envelopes; it does not mean exact RDF-term equality. All 60 requests must qualify before a 45-request measured total can be reported. No 100M candidate outputs have been collected at this stage.

`bundle/provenance.json` maps every selected response to its original attempt and construction method; `bundle/reference/` holds the 68 immutable response files. `runtime/` retains the original and retry matrices, partition queries/responses, fixture, source copies, server commands, logs and metadata snapshots. Owned processes exited and exact owner locks were released. Shared TDB2 data-file metadata matches the initial authorized count probe; only its expected lock-file metadata changed. The NT metadata remained unchanged and no shared lock files or optimizer files were removed or edited.

The planning experiment used the documented [Jena optimizer context setting](https://jena.apache.org/documentation/tdb/optimizer.html) in an owned [query-only Fuseki configuration](https://jena.apache.org/documentation/fuseki2/fuseki-config-endpoint.html). These settings did not recover the missing full query.

`file-sha256.json` binds every archived file except itself. Next: qualify engine outputs against this explicitly heterogeneous-execution, independent Jena reference before comparing performance.
