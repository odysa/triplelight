# Probe membership-set candidate: frozen correctness qualification

Candidate `1cd888103dfdb93643ed158fe6d077e90da27b4b` on `probe-set`, based on
main `4571618`, passes **60/60 frozen oracle checks, 60/60 exact result bags and
all 36 applicable ordered sequences** on BSBM BI10M. This is a host correctness
qualification, not a new performance A/B or a new three-engine VM ranking.

The change replaces repeated fully bound membership probes with a scanned hash
set for patterns containing two constants and one variable bound in every input
row. It requires at least 1,024 input rows, a constants-prefix range no larger
than the input and a range cap of 2^24. The two constants form the index prefix;
the third index column supplies membership values. The result copies matching
input rows in input order, preserving repeated input rows. Unbound inputs and
unresolved constants fall back to the existing path. Scan errors still propagate
through the storage-error path. Source review found no blocker in this scope;
this cohort does not certify every possible BGP shape or general SPARQL conformance.

The isolated 33-file snapshot is built with locked dependencies, one Cargo job
and the release profile. The copied host executable SHA-256 is
`9d58f8c76a07830b3877f305065127f13bd0586277715f0f052e1bbae9548a98`.
The executable is excluded; source, patch, build command/toolchain and build log/
exit are retained. The query-only run uses the existing 10,102,032-triple disk
store, four Rayon threads and a 10,240 MiB cache target on the Mac host, without
a container memory cap.

The original numeric freeze remains
`a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244`.
No validator, tolerance, prior score or raw response is changed. Every request
passes the unchanged strict validator against the retained independent Jena
reference and frozen numerical inputs. Complete RDF term bags and 36 ORDER BY
sequences separately match the previously qualified `71ab35c` responses. Baseline
and Jena files are checked against their published archive hashes. This is a
cross-version/backend correctness comparison, not a speed comparison.

[frozen-postcheck.json](frozen-postcheck.json) verifies all 33 candidate source
files, seven validators, binary hash, 60 query/body/baseline/reference identities,
32 numerical bindings and unchanged store CURRENT/base-manifest hashes.
[baseline-comparison.json](baseline-comparison.json) records every bag/sequence
outcome. The build and client exit zero; the owned server is explicitly stopped
after completion (exit -15). The shared lock is released after all checks pass.

The raw client timings remain with the responses, but this correctness run does
not re-establish Kaiming's separate `ab26` q1/q7 speed observations. The earlier
VM table stays pinned to its actual earlier binary. New integrated performance
claims need their own matched session.

[file-manifest.json](file-manifest.json) binds all retained runtime/source files
by raw hash and length. Larger JSON/body files use lossless gzip with stored and
raw hashes. Verify stored bytes, decompress to the original path, then verify raw
bytes before replay. No database, executable blob or credential is included.
`run.py` and `finish-postcheck.py` retain execution and postcheck logic.
