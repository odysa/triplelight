# DuckDB R2RML and experimental SQLite adapter: tiny qualification

**Neither tested profile is admitted to a full-workload timing comparison.** In the fixed nine-term graph, DuckDB's plain projection returns eight rows and both COUNT(*) and COUNT(DISTINCT ?o) return eight instead of nine. SQLite rejects ten of thirteen generated queries. These are untimed, version-specific qualification observations, not performance scores or a statement about all RDF mappings or SQLite implementations.

## What ran

Thirteen fixed queries ran once through each of two in-process profiles: DuckDB's native execute_sparql function, and SQLite executing the identical sparql_to_sql output with an explicitly supplied Python url_encode function. No SQL was rewritten. All thirteen translations succeeded, yielding 26 preserved engine observations. Version 2 completed normally within its 120-second supervisory cap and closed both connections; source/input/extension postflight hashes match. No dataset import or full benchmark was attempted.

Runtime: macOS ARM64, CPython 3.14.3, DuckDB 1.5.6, SQLite 3.50.4. The signed RDF extension SHA-256 is 95c96225f67ab44c1011017f9221a2f02a17b37c37578e9b8f871c853947b400. DuckDB uses one thread and a 256 MB memory target; this is not a measured RSS cap. Python executable/native DuckDB module identities are recorded. SQLite is built into this interpreter, so its origin and interpreter identities are retained; a separately hashed SQLite library is not claimed. The existing host environment was reused, not reconstructed in a newly pinned Linux image.

The nine one-row R2RML maps denote the unchanged shared consumer-fixture.nt graph, with explicit IRI, string, language, integer subtype, boolean, double and decimal terms. Its GROUP BY result is the original independent Jena JSON. An IRI and a string intentionally share lexical text urn:iri. The other small controls have prospectively declared expectations; they are not new Jena runs. A separate duplicate-input map checks graph-set semantics; the employee map contains two names and one NULL.

## Observations and the adapter boundary

| Control | DuckDB native extension | SQLite on identical generated SQL |
|---|---|---|
| Plain object projection | 8 rows; expected 9 | Syntax error |
| COUNT(*) / COUNT(DISTINCT ?o) | Both 8; expected 9 | Syntax errors |
| Grouped objects/counts | 9 rows, each count 1; necessary lexical/count bag passes | Syntax error |
| DISTINCT objects | 9 rows plus an auxiliary d_o discriminator; offline typed decoding matches all 9 reference terms | Syntax error |
| Explicit isIRI/isBlank/DATATYPE/LANG projection | All 9 typed terms match the reference | Syntax error |
| sameTerm IRI / sameTerm string / language filter | All three count controls pass | Syntax errors |
| Duplicate physical input / employee NULL omission / empty projection | All pass | All three pass |
| OPTIONAL unbound cells and projection | Pass | Syntax error |

The original one-column DISTINCT scalar-shape check reports false because the result includes the additional d_o column. That original verdict remains in the packet. It is an adapter-shape mismatch, not evidence that DISTINCT lost a term: the portable replay decodes the I/D/@ discriminator and verifies the complete typed bag. The explicit typed projection also passes. Ordinary mixed-variable metadata must therefore be described per query shape, rather than assumed absent from the entire extension.

The decisive plain-projection and count failures remain independent of that adapter correction. Their saved generated SQL projects away the discriminator before UNION/counting, leaving eight lexical values. The grouped, DISTINCT and explicit typed-projection SQL retain it and distinguish all nine terms. This is evidence about these fixed translations and this extension binary, not an isolated finding about every underlying source version. It is not repaired by guessing types after COUNT has already returned eight.

SQLite's errors include unsupported generated UNION BY NAME syntax and the tested OPTIONAL form. This adapter deliberately does not translate the DuckDB dialect into SQLite. These observations do not show that SQLite cannot evaluate an equivalent, correctly translated query. The [official extension documentation](https://duckdb.org/community_extensions/extensions/rdf) documents DuckDB-only SQL translation. The archived [source documentation snapshot](https://github.com/nonodename/duck_rdf/blob/badeb50fd12f0004528352bb879f72e055e4dd7b/docs/sparql.md) is a separately identified reference, not proof of the prebuilt extension's matching source commit.

## Retained setup failure and coordination anomaly

The single version-1 attempt stopped during identity capture because the harness assumed _sqlite3 had __file__. No connection or query was created. Its exit, traceback, source and post-failure audit remain in raw/v1 and source-v1. The error was reported as a harness mistake, not an engine failure. Version 2 changed module identity capture and its output/owner namespace; query texts, mappings, checks and limits stayed byte-identical. Kaiming reviewed the narrow change before the separate attempt.

During the version-2 lock window, Kaiming reported that his failed lock-acquisition shell chain nevertheless ran tiny control containers, dropped the VM page cache once and deleted our lock directory (Raft message 83a2317b-3321-4a35-87e5-0e23125bd6e5). Our final owner check found the file absent, so a verified self-release is not claimed. The exact overlap with the brief query child is not established. The attempt makes no isolation, latency, memory-efficiency or cache-causality claim. Both our own engine connections closed and the child exited normally; the unexpected lock deletion and accountable disclosure are retained alongside the results. No rerun replaces that history.

## Registration, replay and next step

V1 source freeze: 62e9a1290bf668635d4a96977469239341d1a21e1566be5d7fba5b73ae23384f; announcement e77f9748-f6bb-47e2-b3f9-f3e9d74f0574. V2 freeze: ff8f09c4620f491e572478d867a0da8dcf2d74ebb2130a18a8f6db070ae12adb; Kaiming approval ae6d29cb-f171-48ac-8d82-5cc65667d011; acquisition announcement b414b288-17e2-4f3e-8518-ad9a4ad78803 preceded execution. These were untimed admission controls, not scoring registrations. The boolean admission flags remain false for both profiles; full_workload_qualified is false.

Run python3 replay.py (Python 3.12+) to extract the compact packet into a temporary directory, verify file/source hashes and replay its typed distinctions and selected failures. No engine, network, archived driver execution or original-machine path is needed. PORTABLE_REPLAY.json records a successful replay. packet.tar.gz preserves both driver revisions, all 39 translation/engine case files, runtime/postflight records, the failed setup, lock receipts and the coordination disclosure. No database blobs, native binaries or credentials are included.

There is no qualified R2RML timing total to add to existing DuckDB typed-triples or embedded benchmark tables. A future SQLite-specific translation or different mapping/compiler is a new profile requiring its own frozen typed-answer admission before timing. This report completes the tested route's admission check; it does not claim the broader full-workload comparison has been delivered.
