# Statistics scan cache isolation: independent 60-query correctness check

The fixed TripleLight candidate `c3a86d465c79bb42a5e1ca0323b8e7bcb1688aa2` passes all 60 frozen BI10M Jena checks. All 60 complete result multisets also match the retained baseline exactly as RDF terms, and all 36 applicable ordered sequences match. This validates the tested query outputs; it is not an embedded performance comparison or an independent verification of the earlier RSS observations.

| Check | Result |
|---|---:|
| Clean release build with --locked | Pass |
| Frozen Jena qualification | 60 / 60 |
| Exact baseline RDF-term multisets | 60 / 60 |
| Ordered baseline sequences | 36 / 36 |
| Source, store and binary identities unchanged | Pass |
| Recorded build/server/supervisor processes absent | Pass |

The Jena gate retains the registered numerical error bounds and expected datatypes; “60 Jena checks” does not mean exact equality to every Jena numeric lexical form. The exact multiset/sequence columns compare the candidate to the retained earlier TripleLight/rdfdb baseline under the same workload. No COUNT datatype or numeric tolerance was added or changed for this candidate.

## Fixed inputs and build

Source: branch experiment/stats-triplelight, commit `c3a86d465c79bb42a5e1ca0323b8e7bcb1688aa2`. A GitHub archive provided a clean snapshot of 43 selected Rust/Cargo source files. The snapshot built with `cargo build --locked --release --bin triplelight -j1`, using an existing agent-owned target cache. The copied binary SHA-256 is `0992a98cebf94d64108c0c01c529e10796d771de798323ebf8361ffafc682c4d`. No candidate source was patched.

The workload is the frozen 60-query BI10M cohort (15 warmup-labelled queries and 45 scored-labelled queries in its original benchmark). All 60 are treated as correctness requests here. Workload manifest SHA-256: `638126af630e952e0ee85a98778c274e64d52f9b0fe6456f5e9c70342e2ba13a`. Numerical freeze: `a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244`. Frozen reference hashes are checked against the previously published value-policy binding, while this candidate uses the original typed numerical gate. The original inputs and historical baseline remain unchanged.

The native server uses the existing agent-owned 10,102,032-triple disk store, loopback only, four Rayon threads and a 10,240 MiB cache configuration. This pinned source predates the optional-server feature change; it was built according to its own manifest. A single candidate server receives requests sequentially under the shared machine lock. This run uses HTTP only to collect complete query responses for correctness. Automatically recorded elapsed values in the generic harness are retained as observations, not presented as an A/B result or substituted for the planned embedded-to-embedded benchmark.

## Attempts and independent replay

The first client attempt failed on import because the isolated validator copy lacked fixture_check.py. The second stopped in the frozen validator's source-identity check because other files in its declared source manifest had not been copied. Neither attempt issued a workload query. Both servers were stopped, and both attempts' driver, logs, statuses and unchanged store/binary records are retained. The fixes restored identical historical dependency files; they did not change the validator or acceptance rule. A preflight then checked every declared source hash and successfully initialized FrozenInputs before starting attempt 3.

Attempt 3 completed all 60 responses. Independent post-run replay checks each response hash, the frozen query/reference identities, the Jena gate and the complete baseline RDF-term bags/sequences. Source files, binary and all store-file hashes agree before/after; recorded processes are absent and the owned lock was released. There were no candidate query failures in this successful attempt. The scratch supervisor's conservative allocation/free-space guards are sampled, not hard quotas.

postchecks.json and verified-results.json contain the audited outcome. attempt3/ holds the successful raw responses and client records. The first two failed setup attempts remain visible at the original root and attempt1-harness-missing-module/ / attempt2/; they have not been silently replaced. validator-source/ and frozen-validator-preflight.json identify the unchanged numerical policy and its dependencies. Historical absolute paths require explicit adaptation for replay.

The candidate executable, target cache, bulk source snapshot and database blobs are not published here; source.json and build-status.json bind their identities. No credentials or private keys are included. SHA256SUMS.json hashes every published file except itself. This is a scoped 10M query-output qualification, with no 100M, memory-speedup or full SPARQL conformance claim.
