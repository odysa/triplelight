# Hash-extend candidate: frozen and scan-orientation qualification

Candidate `5ac99ccbe1a008a381923300920f426fcc766e2a` on `hash-extend`, based on
main `6f9c353`, passes **60/60 frozen oracle checks, 60/60 exact result bags and
all 36 applicable ordered sequences** on BSBM BI10M. Nine additional fixtures
pass exact independent Jena bag comparison and exact main-versus-candidate
sequence comparison. This is correctness qualification, not a new speed A/B.

## Change and scope

For one-constant patterns with one variable bound in every input row and one
free variable, the candidate can scan the `(constant,key,free)` permutation once
and build a key → ordered-values map. It requires at least 1,024 input rows,
a constant range no larger than twice the input and a range cap of 2^24.
Per-key values occupy contiguous ranges in the scanned value vector. Row
expansion follows input order and the free-value index order, retaining repeated
input rows and zero/multiple matches. Repeated-variable and mixed binding shapes
fall back. Scan errors continue through the storage-error path.

Review found no blocker within this scope. The map relies on the Snapshot scan
contract's permutation ordering; the fixtures exercise all six slot orientations
and a memory-scan batch boundary. They do not certify
all BGP/overlay/corruption cases or general SPARQL conformance.

## Frozen cohort and source binding

An isolated 33-file source snapshot is built with locked dependencies, one Cargo
job and the release profile. Copied host executable SHA-256:
`73694951ae31723dc8ca4c4b6c7b5807f3892569dd389ec5cdb4950561ed2e56`.
Source, patch, toolchain/build command and build log/exit are retained; the
executable itself is excluded. The query-only cohort uses the existing
10,102,032-triple disk store, four Rayon threads and a 10,240 MiB cache target on
the Mac host, without a container memory cap.

The original numeric freeze remains
`a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244`.
No validator, tolerance, old score or raw response changes. Every request passes
the unchanged strict validator against independent Jena references and frozen
numeric inputs. Complete RDF bags and 36 ORDER BY sequences separately match
the previously qualified `71ab35c` responses. Baseline and Jena files are checked
against their published archive hashes. This cross-version/backend comparison
concerns correctness, not speed.

[frozen-postcheck.json](frozen-postcheck.json) verifies all 33 candidate source
files, seven validators, binary hash, 60 query/body/baseline/reference identities,
32 numeric bindings and unchanged store CURRENT/base-manifest hashes. Build and
client exit zero; the owned cohort server is explicitly stopped (exit -15).

## Nine independent fixtures

The 38,912-triple fixture graph and queries are retained under orientation-fixture/.
Each query supplies 4,096 input rows; input keys repeat. Each positive orientation
has 6,144 triples for its constant, within the 8,192-row selection bound. The
memory scanner emits chunks of 4,096, so the scan crosses a batch boundary. Main and candidate use the same serially loaded
in-memory graph for these fixtures. Main uses the qualified `6e14248` executable,
whose only difference from main `6f9c353` is README.md; binary identities are bound
before/after the fixture.

| Case | Expected and Jena rows |
|---|---:|
| Six `(constant,key,free)` slot permutations, each | 6,144 |
| Repeated-variable fallback | 2,048 |
| Mixed free-variable binding fallback | 5,324 |
| Mixed key-variable binding fallback | 12,287 |

Main, candidate and Jena ARQ 6.2 run serially, never concurrently. The independent
Jena result counts match the predeclared expected row counts. Every main/candidate
result bag matches Jena. The fixture queries have no ORDER BY, so Jena's arbitrary
sequence is recorded as an observation **and is not a gate**; candidate sequences
must instead match main exactly, validating the claimed probe-order preservation.
All nine pass that additional gate.

[orientation-fixture/summary.json](orientation-fixture/summary.json) contains the
matrix, process exits and input-hash checks. Queries/data/expected counts are
hash-bound before responses; ARQ script/core JAR identities, commands, raw outputs
and errors are retained. The owned Rust fixture servers are explicitly stopped,
and all nine Java processes exit zero. The shared lock is released only after
all children and all postchecks finish.

Kaiming's host A/B `ab28` is separate evidence; its proposed q1 speedup is not
re-established by this run. The latest three-engine VM table remains pinned to
`1cd8881`. An integrated speed claim needs a fresh matched session.

[file-manifest.json](file-manifest.json) binds every retained runtime/source/fixture
file by raw hash and length. Larger JSON/body files use lossless gzip with stored
and raw hashes. Verify stored bytes, decompress to the original path, then verify
raw bytes before replay. No executable blob, database or credential is included.
