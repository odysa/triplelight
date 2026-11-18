# Filter memo candidate: frozen correctness qualification

Candidate `9c9a13f0cdfc7a0fcfeb40e1763b974058423a38` on `filter-memo`, based
on main `cc15f306`, passes **60/60 frozen oracle checks, 60/60 exact result bags
and all 36 applicable ordered sequences** on BSBM BI10M. This is a host
correctness check, not a performance A/B or a new same-session VM ranking.

The candidate memoizes the fallback evaluation of a one-variable BGP filter per
term ID when the compiled numeric path does not apply. EXISTS is excluded, and
the map is local to each filter application. Source review found no AST-pointer
key lifetime issue or dependence on another bound column in this scope. Currently
supported scalar expressions are deterministic; NOW/RAND/UUID/BNODE remain
unsupported. This review does not certify future nondeterministic functions or
general SPARQL conformance.

A source snapshot of 33 files is pinned to the candidate. A single-job, locked
release build produces the copied host executable with SHA-256
`66cca66984653a8c3c1043f23bdafd9ee5ccc31c01a8c49bfca872622a1c7e0a`.
Source files, build command/toolchain, patch and build exit are retained. The
executable itself is excluded. The server uses the existing read-only query path
against the same 10,102,032-triple disk store with four Rayon threads and a
10,240 MiB cache target. This is the host, without a container memory cap.

The original numeric freeze remains
`a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244`.
Every query is checked against the retained independent Jena reference, using the
unchanged strict validator and the frozen numerical bounds. Separately, complete
RDF term bags and the 36 ORDER BY sequences match the previously qualified
`71ab35c` responses. The old baseline and Jena files are checked against their
published archive hashes. This cross-version/backend correctness comparison is
not a timing comparison. No validator, tolerance or prior outcome is changed.

[frozen-postcheck.json](frozen-postcheck.json) binds the result: all 33 candidate
sources, seven validator sources, binary hash, 60 query/body/baseline/reference
identities and 32 numerical bindings verify after execution. The client exits
zero; the owned server is explicitly terminated after completion (exit -15).
The store CURRENT/base-manifest hashes are unchanged. The lock is released only
after the build, client and server exit and these checks pass.

[baseline-comparison.json](baseline-comparison.json) has each complete bag and
sequence result; queries/ retains every raw response, status and client timing.
[run.py](run.py), source.json, binary.json, runtime.json and build-command.json
record the executable and execution path. Raw timings are retained as observations
of this correctness run; they do not support the proposed 2.1× host improvement,
which belongs to Kaiming's separate A/B evidence.

[file-manifest.json](file-manifest.json) records every retained file's raw hash
and length. Larger JSON/body files use lossless gzip, with separate stored hashes.
Verify stored bytes, decompress to the original path, then verify raw bytes before
replaying. No database blob, binary, credential or independent reference answer
is rewritten. The candidate is ready for the engine owner's integration decision
within this tested scope; a new performance claim needs its own matched session.
