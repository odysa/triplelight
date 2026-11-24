# TripleLight stable release checklist

Record the release tag, full source commit, supported targets, artifact digests and evidence links on this page for each stable release. A gate passes when its owner supplies the listed evidence for the exact release candidate. A package is supported on a platform after a clean install of that package passes there.

## Compatibility policy for the first stable release

The release owner approves this policy before tagging.

- **Public APIs.** Identify the supported Rust APIs, C ABI symbols and structures, CLI commands and exit codes, and language package APIs. Reserve breaking changes to a stable public contract for a major release. Publish a migration guide with any breaking change. Treat documented RDF term encodings, named-graph behavior, transaction boundaries, and error types as part of the contract.
- **Store reads and upgrades.** Read every store format published under the current stable major release. If a later major release drops an older format, provide a tested export or migration path in the previous release. Record the oldest format the candidate reads and test every supported transition, including a store with uncheckpointed updates. A supported store opens with the same data and receives any format upgrade at compaction. An older reader must reject an unsupported manifest or WAL format before making any change to the directory. Downgrades use a verified backup or export and re-import.
- **Feature compatibility.** Record which reader features each fixture requires. Include named graphs and feature-gated RDF 1.2 stores. Publish the supported operating systems and architectures, runtime minimums, and which combinations support persistent stores.
- **Breaking-change detection.** Compare the public Rust surface and C ABI with the previous stable artifact. Run examples and contract tests with older clients against the new release where binary compatibility is promised. Record intentional changes and the release version that permits them.

See the [embedded API](https://github.com/odysa/triplelight/blob/main/docs/embedded/README.md), [persistent formats](https://github.com/odysa/triplelight/blob/main/docs/persistence.md), and [CI workflow](https://github.com/odysa/triplelight/blob/main/.github/workflows/ci.yml).

## Backup and restore

Qualify an offline directory backup for the first stable release. The store owner stops writers and closes all store and snapshot handles before copying; hold the application closed until the copy is complete. A copy includes the entire store directory so the active base, graph catalog, CURRENT and WAL stay together. Store the backup digest, TripleLight version and enabled features with the copy.

Restore into a fresh directory and open it with a compatible build. Verify its active base, then open the store and run typed queries to verify the recovered data, including committed WAL changes. Compare default and named graphs, and check that an acknowledged deletion stays deleted. Validate the backup through a restore drill before using it as the upgrade rollback point. Retain the original backup through the upgrade.

| Gate | Owner | Pass condition | Candidate evidence |
| --- | --- | --- | --- |
| Backup and restore | Storage | Restore a known store whose active base has pending WAL inserts and deletes. Exact typed queries, graph catalog and verifier match. Repeat after compaction. | Pending |
| Backup under invalid input | Storage | A corrupt or incomplete copy fails safely with a storage error. Restore operates on a fresh directory and preserves the source backup. | Pending |
| Upgrade and rollback | Storage and release | Each supported old fixture opens and compacts correctly. Unsupported older readers refuse before writing. The restore drill recovers the pre-upgrade state on the prior release. | Pending |

## Storage and query failure gates

Use disposable stores and injected I/O faults. Keep the expected committed state and every observed exit status with the receipt. Reopen with the ordinary embedded API, exhaustively verify the base, and compare answers after each failure.

| Gate | Owner | Pass condition | Candidate evidence |
| --- | --- | --- | --- |
| WAL crash boundaries | Storage | Crash before append, after append and before sync, and after acknowledgement. Acknowledged updates survive. Replay a torn tail without exposing a partial update. Repeat with named-graph updates. | Pending |
| Compaction crash boundaries | Storage | Crash before and after the CURRENT switch and WAL truncation. Restore exact committed answers, and verify the preserved base. A snapshot held through a successful compaction continues to see its original state. | Pending |
| Disk-full and I/O errors | Storage | Inject a write or sync failure at WAL append, base creation and CURRENT publication. Return an actionable error. Reopen and verify committed state and retry the operation. | Pending |
| Mixed read and write activity | Storage and executor | Queries see coherent snapshots across updates. Old and new readers see their respective states through compaction. Record peak disk use while readers hold the old base. | Pending |
| Query cancellation and budget | Executor | Cancel a running query and enforce a deadline while scanning, joining, grouping, sorting and producing output. At the supported memory budget, finish by spilling or return a documented resource error. Release scratch files and allow an immediate subsequent query. | Pending |
| Result streaming | Embedded APIs | Produce a first Arrow batch before a large compatible query completes. A slow or cancelled consumer leaves memory bounded and releases its query and snapshot. Type and order checks agree with the materialized API. | Pending |
| Standards and feature coverage | Query and SHACL | Record exact committed CI gates and exclusions for SPARQL, RDF and the supported inference and validation regimes. Verify matching typed results and errors on supported bindings. | Pending |

## Package and platform qualification

The matrix below proposes first-release targets based on existing source support and release preparation. Fill the actual package names, registry versions and supported runtimes from the tagged files. Claims follow install receipts from the exact candidate, including the difference between native and emulated tests.

| Surface | Candidate targets | Receipt required | Owner |
| --- | --- | --- | --- |
| Rust crates | Native macOS ARM64 and Linux ARM64 and x86-64 with the pinned Rust minimum | Publish-order dry run, clean downstream build, in-memory and durable open, query, update and reopen | Rust release |
| Python wheels and sdist | macOS ARM64 and manylinux ARM64 and x86-64, plus a source distribution | Twine, notices, clean installs and typed persistence smoke on the minimum supported CPython. Rebuild and install from the sdist separately. | Python release |
| Node.js native package | Platform targets declared by the tagged npm package | Pack and install in a clean directory with each supported Node runtime and native artifact. Exercise persistent open and typed results. | npm release |
| WebAssembly package | The browser and Node environments named by the tagged WASM package | Pack and clean install. Run an in-memory import, typed query and update. | npm release |
| C, Go and Java | Native targets declared for these releases | Build a downstream consumer from published files. Verify C ABI, owned result release, and Arrow stream handling. Run Java on the documented runtime minimum. | Bindings |
| CLI and native release files | The macOS and Linux targets on the release page | Check digests, execute a clean start, create and verify a store, query and reopen, smoke each supported opt-in feature. | Release owner |

Before publishing, verify licenses, notices, checksums and provenance against the tag. After owner approval, publish in dependency order and repeat fresh installs by version from each registry or release download. Record the exact artifacts, platform, runtime, verifier output and install receipt. Keep the published target list aligned with those receipts.

The 0.1.0 dry runs are preparation evidence. Rebuild and repeat the checks from the tag selected for publishing.
