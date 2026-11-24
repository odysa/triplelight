# TripleLight stable release checklist

Record the release tag, full source commit, supported targets, artifact digests and evidence links on this page for each stable release. A gate passes when its owner supplies the listed evidence for the exact release candidate. A package is supported on a platform after a clean install of that package passes there.

## Compatibility policy for the first stable release

Jianlin Su is the release owner until the team names a successor. The release owner records approval of this policy before tagging.

- **Public APIs.** Identify the supported Rust APIs, C ABI symbols and structures, CLI commands and exit codes, and language package APIs. Reserve breaking changes to a stable public contract for a major release. Publish a migration guide with any breaking change. Treat documented RDF term encodings, named-graph behavior, transaction boundaries, and error types as part of the contract.
- **Store reads and upgrades.** Read every store format published under the current stable major release. If a later major release drops an older format, provide a tested export or migration path in the previous release. Record the oldest format the candidate reads and test every supported transition, including a store with uncheckpointed updates. A supported store opens with the same data. A base-format upgrade happens at compaction; a WAL or directory format change writes its fence into CURRENT before the first incompatible write (for example `walcrc-1`, written before the first WAL batch, so the first write after an upgrade fences the store). An older reader must reject an unsupported manifest or WAL format before making any change to the directory. Downgrades use a verified backup or export and re-import.
- **Feature compatibility.** Record which reader features each fixture requires. Include named graphs and feature-gated RDF 1.2 stores. Publish the supported operating systems and architectures, runtime minimums, and which combinations support persistent stores.
- **Breaking-change detection.** Compare the public Rust surface and C ABI with the previous stable artifact. Run examples and contract tests with older clients against the new release where binary compatibility is promised. Record intentional changes and the release version that permits them.

See the [embedded API](https://github.com/odysa/triplelight/blob/main/docs/embedded/README.md), [persistent formats](https://github.com/odysa/triplelight/blob/main/docs/persistence.md), and [CI workflow](https://github.com/odysa/triplelight/blob/main/.github/workflows/ci.yml).

## Backup and restore

The first stable release qualifies offline directory backups only. Online snapshot export while writers run is outside this release’s scope. The store owner stops writers and closes all store and snapshot handles before copying; hold the application closed until the copy is complete. A copy includes the entire store directory so the active base, graph catalog, CURRENT and WAL stay together. Store the backup digest, TripleLight version and enabled features with the copy.

Restore into a fresh directory and open it with a compatible build. Verify its active base, then open the store and run typed queries to verify the recovered data, including committed WAL changes. Compare default and named graphs, and check that an acknowledged deletion stays deleted. Validate the backup through a restore drill before using it as the upgrade rollback point. Retain the original backup through the upgrade.

| Gate | Owner | Pass condition | Candidate evidence |
| --- | --- | --- | --- |
| Backup and restore | Alex Chi | Restore a known store whose active base has pending WAL inserts and deletes. Exact typed queries, graph catalog and verifier match. Repeat after compaction. | Pending |
| Backup under invalid input | Alex Chi | A corrupt or incomplete copy fails safely with a storage error. Restore operates on a fresh directory and preserves the source backup. | Pending |
| Upgrade and rollback | Alex Chi and Jianlin Su | Each supported old fixture opens and compacts correctly. Unsupported older readers refuse before writing. The restore drill recovers the pre-upgrade state on the prior release. | Pending |

## Storage and query failure gates

Use disposable stores and injected I/O faults. Keep the expected committed state and every observed exit status with the receipt. Reopen with the ordinary embedded API, exhaustively verify the base, and compare answers after each failure.

| Gate | Owner | Pass condition | Candidate evidence |
| --- | --- | --- | --- |
| WAL crash boundaries | Alex Chi | Crash before append, after append and before sync, and after acknowledgement. Acknowledged updates survive. Replay a torn tail without exposing a partial update. Repeat with named-graph updates. | Done: `store::fault_tests` (#256, `f31b5d65`), 12/12 local CI at `d00c5362`. Mid-log WAL damage is now told apart from a torn tail by WAL batches with a hash (#282, `a7c5825f`, 13/13 local CI): open refuses damage followed by a valid batch and leaves the WAL unchanged, while damage in the final batch is discarded as a torn tail (`store::wal_batch_tests`). |
| Compaction crash boundaries | Alex Chi | Crash before and after the CURRENT switch and WAL truncation. Restore exact committed answers, and verify the preserved base. A snapshot held through a successful compaction continues to see its original state. | Done: `store::fault_tests` (#256, `f31b5d65`), 12/12 local CI at `d00c5362`. |
| Disk-full and I/O errors | Alex Chi | Inject a write or sync failure at WAL append, base creation and CURRENT publication. Return an actionable error. Reopen and verify committed state and retry the operation. | Done: `store::fault_tests` (#256, `f31b5d65`), 12/12 local CI at `d00c5362`. |
| Mixed read and write activity | Alex Chi and Kaiming He | Queries see coherent snapshots across updates. Old and new readers see their respective states through compaction. Record peak disk use while readers hold the old base. | Pending |
| Planner statistics after updates | Alex Chi | After a committed update and before compaction, keep cost-based planning active. Use a repeated same-session comparison with an update that preserves the selected answers. Require the median post-update time to stay within 1.20× of pre-update for each registered analytical query, including agg-WatDiv C3. | Laptop evidence: #263 (`d2ceb05e`) and #264 (`5a10bf58`), 12/12 local CI each; after one update all 15 m000 BSBM-10 BI queries are within 1.09× of their pre-update medians (`docs/stats-after-writes-v1/`). In a separate statistics-only comparison, agg-WatDiv C3 is within 1.02×. Repeat on the rental before release. |
| Query cancellation and budget | Kaiming He | Cancel a running query and enforce a deadline while scanning, joining, grouping, sorting and producing output. At the supported memory budget, finish by spilling or return a documented resource error. Release scratch files and allow an immediate subsequent query. | Pending |
| Result streaming | WindowsXP | Produce a first Arrow batch before a large compatible query completes. A slow or cancelled consumer leaves memory bounded and releases its query and snapshot. Type and order checks agree with the materialized API. | Pending |
| Standards and feature coverage | Jianlin Su | Record exact committed CI gates and exclusions for SPARQL, RDF and the supported inference and validation regimes. Verify matching typed results and errors on supported bindings. | Pending |

## Package and platform qualification

The matrix below proposes first-release targets based on existing source support and release preparation. Fill the actual package names, registry versions and supported runtimes from the tagged files. Claims follow install receipts from the exact candidate, including the difference between native and emulated tests.

| Surface | Candidate targets | Receipt required | Owner |
| --- | --- | --- | --- |
| Rust crates | Native macOS ARM64 and Linux ARM64 and x86-64 with the pinned Rust minimum | Publish-order dry run, clean downstream build, in-memory and durable open, query, update and reopen | Kaiming He |
| Python wheels and sdist | macOS ARM64 and manylinux ARM64 and x86-64, plus a source distribution | Twine, notices, clean installs and typed persistence smoke on the minimum supported CPython. Rebuild and install from the sdist separately. | Jianlin Su |
| Node.js native package | Platform targets declared by the tagged npm package | Pack and install in a clean directory with each supported Node runtime and native artifact. Exercise persistent open and typed results. | WindowsXP |
| WebAssembly package | The browser and Node environments named by the tagged WASM package | Pack and clean install. Run an in-memory import, typed query and update. | WindowsXP |
| C, Go and Java | Native targets declared for these releases | Build a downstream consumer from published files. Verify C ABI, owned result release, and Arrow stream handling. Run Java on the documented runtime minimum. | WindowsXP and Alex Chi |
| CLI and native release files | The macOS and Linux targets on the release page | Check digests, execute a clean start, create and verify a store, query and reopen, smoke each supported opt-in feature. | Jianlin Su |

Before publishing, verify licenses, notices, checksums and provenance against the tag. After owner approval, publish in dependency order and repeat fresh installs by version from each registry or release download. Record the exact artifacts, platform, runtime, verifier output and install receipt. Keep the published target list aligned with those receipts.

The 0.1.0 dry runs are preparation evidence. Rebuild and repeat the checks from the tag selected for publishing.
