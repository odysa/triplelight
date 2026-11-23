# Embedded comparator laptop checks

These macOS ARM64 checks use a nine-term fixture and Jena reference results. They establish which cases can proceed to workload qualification. The checks record no comparative timings.

## DuckDB rdf

DuckDB 2.0.1 loaded community extension rdf v2.9.0 with signature enforcement enabled. The installed binary matched the pre-registered SHA-256. The first attempt loaded by explicit path without the community install metadata and stopped at signature verification. The retry installed from the community repository into a private directory and recorded the binary and metadata hashes before running the fixture.

Nine of 13 cases passed the frozen checks. Plain projection returned eight rows from nine RDF terms. An IRI and a string shared the lexical value `urn:iri`; the generated plain-projection SQL deduplicated the single lexical column. Both COUNT forms returned eight where the frozen reference expects nine.

The DISTINCT response carried nine rows in two columns, a lexical value and a tag. It failed the frozen one-column check. Decoding the tags and comparing the resulting multiset against the Jena terms gives nine exact typed matches. The explicit typed-discriminator query also matched the nine Jena terms. That query uses an explicit projection of term kind, datatype and language fields.

DuckDB rdf is eligible for descriptive workload evaluation. The workload evaluation will record failures by class and admit only exact passing requests to timing. The explicit typed-discriminator query remains a separate variant.

## RDF Fusion

RDF Fusion 0.3.1 built with Rust 1.100.0 and a locked dependency tree. A fresh local DeltaQuads store loaded nine quads. A separate process reopened and counted nine. A third process returned all nine typed Jena bindings with the same multiplicities, lexical values, datatypes and language tags. Explicit and implicit `xsd:string` were treated as equivalent. The six store files had identical digests after load, reopen and query.

The first attempt built and loaded the fixture. The wrapper then stopped because its reopen guard checked the wrong directory. The retry corrected the guard to check `log/_delta_log` and used a fresh store. The two attempts have separate receipts.

## Replaying the evidence

Extract `packet.tar.gz` into a new directory and run `python3 -IB replay.py` there to verify the receipt hashes and replay the DuckDB fixture, the tagged DISTINCT comparison and the RDF Fusion result comparison. The replay uses saved data and the Python standard library. The raw hashes cover local receipts before path redaction; `SHA256SUMS` covers the redacted files.
