# Batched EXISTS candidate: frozen and independent fixture qualification

Candidate `6e1424893338de2bd53202675cfc319a3b5d20cb` on `exists-batch`, based on
main `1cd8881`, passes **60/60 frozen oracle checks, 60/60 exact bags and all
36 applicable ordered sequences** on BSBM BI10M. It also matches main and
independent Jena on complete bags and ordered sequences for **all 12 small
correlation/fallback fixtures**. This is correctness qualification, not a speed A/B.

## Actual scope and review

A filter expression exactly EXISTS P or NOT EXISTS P, with at least 64 input rows,
may evaluate P once on distinct representatives of the input's correlated
bindings. P is restricted to BGPs, joins and EXISTS-free filters. Its variables
include those read only in inner FILTER expressions. Input rows must bind the
same subset of those variables; mixed binding patterns fall back. Representative
rows retain the correlated columns, and the result keeps original input rows by
membership of their correlation key, preserving order and multiplicity.

The call is **also reachable for plain BGP input** when the earlier filter branch
does not return; it is not restricted to non-BGP input. This scope detail was
reported to the owner before validation and is explicitly exercised below.
Other graph operators such as OPTIONAL are excluded by the distributivity guard.
Source review found no blocker within this tested scope. This is not a general
SPARQL conformance certificate, proof for all correlation shapes or validation
of future nondeterministic function support.

## Frozen cohort and source identity

An isolated 33-file snapshot is built with locked dependencies, one Cargo job
and the release profile. Copied host executable SHA-256:
`2d8a16d808de229112ceb65e0e7a5dfaefe3ff11ddffcf0b677e99cd13d3a1e5`.
The executable itself is excluded; source, patch, toolchain/build command and
log/exit are retained. The query-only cohort uses the existing 10,102,032-triple
disk store, four Rayon threads and a 10,240 MiB cache target on the Mac host,
without a container memory cap.

The original numeric freeze remains
`a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244`.
No validator, tolerance, old score or raw answer is changed. The unchanged strict
validator checks every request against independent Jena references and frozen
numeric inputs. Separately, complete RDF bags and 36 ORDER BY sequences match
the previously qualified `71ab35c` responses. Jena and baseline files are verified
against their published archive hashes. This cross-version/backend comparison
concerns correctness, not speed.

[frozen-postcheck.json](frozen-postcheck.json) verifies all 33 candidate sources,
seven validators, binary hash, 60 query/body/baseline/Jena identities, 32 numeric
bindings and unchanged store CURRENT/base-manifest hashes. Build and client exit
zero; the owned cohort server is explicitly terminated after completion (exit -15).

## Twelve independent fixture checks

The 74-triple input and query texts are under correlation-fixture/. Each VALUES case supplies 64 outer rows; the two plain-BGP cases supply 72.
Expected row counts are written before execution.
All tests project the original rows, so complete bags and ORDER BY sequences
check multiplicity and correlation, rather than checking only a final count.

| Case | Expected/Jena rows | Purpose |
|---|---:|---|
| bound-exists | 32 | Repeated correlated keys |
| bound-notexists | 32 | Anti-membership and input multiplicities |
| empty-key-exists | 64 | Constant-only true EXISTS body |
| empty-key-notexists | 64 | Constant-only false body with NOT EXISTS |
| mixed-bound | 43 | Bound/unbound mixture takes fallback |
| filter-only-outer | 32 | Outer variable read only by inner FILTER |
| join-body | 32 | Joined body and correlated inner FILTER |
| optional-fallback | 32 | Non-distributive body falls back |
| plain-bgp | 36 | Actual reachable BGP input scope |
| plain-bgp-outer-filter | 5 | BGP input plus outer-only filter variable |
| all-unbound | 64 | Uniform unbound variables and empty correlation key |
| empty-body | 64 | EXISTS identity body |

Main `1cd8881`, candidate and Jena 6.2.0 run **serially**, one engine at a time.
The two Rust fixture servers load only the tiny input, with one Rayon thread,
serial load and statistics off. Each exits after explicit shutdown. Jena ARQ runs
one query per owned process with a 512 MiB heap cap and one active processor;
script/jar identities and commands/exits are retained. Each engine's complete
bags **and all ordered sequences** match Jena for all 12 cases. The separate
[fixture summary](correlation-fixture/summary.json) records every outcome and
owned server exit; input/query hashes match afterward. `run_fixture.py` retains
the execution/comparison logic. No fixture changes the frozen BI rule.

The shared lock is released only after the cohort, all fixture processes and
postchecks complete. Raw cohort timings remain retained as observations, but
Kaiming's claimed q7 2.1× improvement is a separate A/B result and is not
re-established here. The latest three-engine VM table stays pinned to `1cd8881`.
A new integrated performance claim needs its own matched session.

[file-manifest.json](file-manifest.json) binds every retained runtime/source/fixture
file by raw hash and length. Larger JSON/body files use lossless gzip with stored
and raw hashes. Verify stored bytes, decompress to the original path and verify
raw bytes before replay. No database, executable blob or credential is included.
