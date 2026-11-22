# TripleLight architecture

TripleLight is an embedded RDF database for analytical SPARQL on one machine. An application opens a directory (or an
in-memory database) through a library call, and every query runs in the application's process. This document
describes the current design; the measured results are in [docs/report/evaluation.md](docs/report/evaluation.md)
and [docs/benchmark-overview.md](docs/benchmark-overview.md).

## Crates

| Crate | Role |
|---|---|
| `triplelight-rdf` | RDF term types (IRIs, blank nodes, literals, variables, triples, quads) and their text parser. A port of `oxrdf` 0.3.5 with identical behaviour. |
| `triplelight-iri` | IRI validation and relative-reference resolution, matching `oxiri`. |
| `triplelight-core` | Term IDs and permutations (`model`), the `Snapshot`/`Dictionary` traits the engine is written against (`storage`), and the N-Triples/N-Quads and Turtle/TriG parsers. |
| `triplelight-sparql` | SPARQL 1.1 query and update parser and algebra. |
| `triplelight-plan` | Statistics (characteristic sets, predicate counts, sketches) and join-order search. |
| `triplelight-exec` | The query executor: joins, filters, aggregation, property paths, result serialization. |
| `triplelight-storage` | The in-memory snapshot, the on-disk snapshot and its builders, the write-ahead log, overlays and compaction. |
| `triplelight` (root) | The embedded API (`embedded`, `async_embedded`), the CLI and the optional SPARQL HTTP server. |

The language bindings (`bindings/python`, `node`, `wasm`, `c`, `go`, `java`) wrap the embedded API. The WebAssembly
build is in-memory only; the others open persistent stores (Unix).

No Oxigraph crate is a runtime dependency. The parsers and term types were written to accept and produce exactly
what `oxttl`, `oxiri` and `oxrdf` did, and differential tests against those crates (dev-dependencies only) keep it
that way. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Terms and IDs

Every term is a 64-bit `TermId`. The top four bits give its kind (IRI, blank node, literal, and inline string). In the
in-memory store, a plain string of up to seven bytes is encoded in the ID itself, so operators can compare it without
a lookup; its text is still stored once in the term arena, like every other term. On disk,
current stores (`RDFSNAP4`/`RDFSNAP5`) keep one front-coded dictionary file in ID order. Within a kind, ID order is
term-text order, so the same file serves lookups by ID and by term. Operators compare, join and group IDs; a term is
decoded only when a filter, a string function or the result needs its text.

## Storage

A **base snapshot** is immutable. It holds the triples in sorted orders (SPO, SOP, PSO, POS, OSP, OPS) as
compressed, checksummed blocks, read on demand through decoded-block caches that share one memory
budget. A dataset with named graphs (`RDFSNAP5`) adds six graph-first quad orders and a catalog of graphs. SOP and
OPS can be served exactly from OSP, POS and SPO (`derived_order.rs`, task #73), which lets a future `RDFSNAP6` base
omit them (#119).

**Updates** are appended to a write-ahead log with group commit and synced before the call returns. They form an
**overlay** of inserts and deletes over the base. Readers pin a snapshot (base plus overlay generation), so a query
never sees part of an update. **Compaction** writes a new base by external sort and switches to it atomically;
readers keep their old snapshot until they drop it.

**Building** a disk store from N-Triples uses an external-sort builder (`--build-external`, or automatically when
the input is large). Small inputs take an in-memory path that writes the older two-file dictionary (`RDFSNAP3`) until
the first compaction. `triplelight verify` checks a base: block checksums, that every order holds the same triples,
that every ID resolves, and the dictionary and graph catalog.

Details and formats: [docs/persistence.md](docs/persistence.md).

## Query processing

1. **Parse** SPARQL into an algebra (`triplelight-sparql`). Features the engine does not implement (`SERVICE`,
   `LOAD`, `BNODE(string)`) are refused with an "unsupported" error rather than answered partially.
2. **Encode** constants through the dictionary. A constant that is not in the data makes its pattern empty.
3. **Order joins.** By default the executor orders a basic graph pattern greedily: at each step it takes the
   pattern with the smallest exact index-prefix count for the current (sampled) binding, and penalizes patterns not
   connected to those already chosen. Eligible cyclic patterns use leapfrog triejoin with its own variable-order
   choice. The planner's cost-based binary order replaces the greedy one only when statistics are available, the
   pattern has two to seven triples, its input is a single all-unbound row in the default data context, and the
   estimated greedy work is at least 100,000. The planner uses characteristic sets and predicate counts
   (Neumann and Moerkotte, ICDE 2011), subset dynamic programming,
   and a cost-based variable order for trie joins. Disk stores persist the statistics next to the base, so a reopened
   store has them at once; otherwise they are built in the background.
4. **Execute** on IDs: leapfrog triejoin for cyclic patterns, index nested-loop (bind) joins, filter pushdown, hash
   aggregation, top-k for `ORDER BY ... LIMIT`, and index-count shortcuts for `COUNT`. Property paths, `OPTIONAL`,
   `MINUS`, `EXISTS`, subqueries and named graphs (`GRAPH`, `FROM`, `FROM NAMED`) are supported.
5. **Return** results, which are materialized before output: SPARQL JSON, owned rows in each binding's types, or
   Arrow C streams from the C, Go and Java bindings.

Optimizer details: [docs/optimizer.md](docs/optimizer.md).

## Correctness

- Every benchmark compares TripleLight's answers with Apache Jena under that benchmark's declared checks (for
  example canonical multisets, or projected columns and `ORDER BY` where the benchmark registers them).
- CI requires at least 323 passing tests from the W3C SPARQL 1.1 query suite ([ci/w3c_floor.json](ci/w3c_floor.json)).
- Numbers: `xsd:decimal` arithmetic is exact within 96 bits and `xsd:integer` within 64 bits; results outside that
  range are errors.
- Storage changes are gated on byte-identical stores and on reading stores written by earlier versions.

## Known limits

- Building a disk store is mostly single-threaded and needs tens of gigabytes of scratch disk at 100M triples.
- Results are materialized before output.
- One process per store directory, enforced by a file lock.
- `LOAD`, `SERVICE`, `BNODE(string)` and blank-node graph names are not supported.

The original design plan (v0) is preserved in git history.
