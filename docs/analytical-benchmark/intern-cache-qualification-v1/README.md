# Intern cache candidate: frozen correctness qualification

Candidate `4571618f65107c9d4777680511e6757247fad405` on `q3-next`, based on
main `9c9a13f`, passes **60/60 frozen oracle checks, 60/60 exact result bags and
all 36 applicable ordered sequences** on BSBM BI10M. This covers only the
per-query intern cache. The parked merge-join WIP `f1233de` is excluded.

The change consults the per-query term→ID map before a stored dictionary lookup,
and caches stored-ID hits there. Scratch IDs still use the separate term vector
for allocation and resolution. The snapshot is fixed for the query, and cached
terms retain their previous stored or scratch IDs. Uncached storage errors still
abort through the existing error path. Review found no scope or ID-mapping
blocker in this change. This is not a general SPARQL conformance certification.

An isolated 33-file source snapshot is built with locked dependencies, one Cargo
job and the release profile. The copied host executable SHA-256 is
`b123e25e36982e0b577cc8cff921274b7ba70e767a097fbc209bc0e2d0df2cd7`.
The binary itself is excluded; source, patch, build command, toolchain and build
log/exit are retained. The query-only run uses the existing 10,102,032-triple disk
store, four Rayon threads and a 10,240 MiB cache target on the Mac host, without a
container memory cap.

The original numeric freeze remains
`a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244`.
No validator, tolerance, raw response or prior score is changed. Every request
passes the unchanged strict validator against the retained independent Jena
reference and frozen numerical inputs. Complete RDF term bags and all 36 ORDER BY
sequences separately match the previously qualified `71ab35c` responses. Baseline
and Jena files are verified against their published archive hashes. This
cross-version/backend comparison concerns correctness, not speed.

[frozen-postcheck.json](frozen-postcheck.json) records all 33 candidate sources,
seven validator sources, binary, 60 query/body/baseline/reference identities,
32 numerical bindings and unchanged store CURRENT/base-manifest hashes.
[baseline-comparison.json](baseline-comparison.json) gives each bag/sequence result.
The build and client exit zero. The owned server is explicitly terminated after
completion (exit -15); the shared benchmark lock is released after all checks.

The raw client timings are retained with the responses, but this is **not a
performance A/B**. Kaiming's claimed host q3 improvements belong to separate
`ab24`/`ab25` sessions and are not re-established here. The earlier three-engine
VM table remains pinned to its earlier binary. A speed comparison for a new
integrated binary needs a fresh matched session.

[file-manifest.json](file-manifest.json) binds the retained runtime/source files
by raw hash and byte length. Larger JSON/body files use lossless gzip with stored
and raw hashes. Verify stored bytes, decompress to the original path, then verify
raw bytes before replay. No database, executable blob or credential is included.
`run.py` and `finish-postcheck.py` retain the execution and postcheck logic.
