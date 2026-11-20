# Oxilite 0.10.0 Linux ARM64: tiny platform qualification

**All 24 fixed native-answer observations pass**: eight cases each in memory, disk-load and separate-process disk-reopen. Each mode reports nine quads. This is a separate Linux platform profile after the [macOS tiny qualification](../oxilite-tiny-qualification-v1/README.md); the results are not pooled. There are no full-workload imports, workload admission or comparative timings.

| Fixed case | Memory | Disk-load | Disk-reopen |
|---|---|---|---|
| Grouped strict RDF-term/count bag | pass | pass | pass |
| Plain strict object bag | pass | pass | pass |
| DISTINCT strict object bag | pass | pass | pass |
| COUNT(*) as xsd:integer 9 | pass | pass | pass |
| COUNT(DISTINCT) as xsd:integer 9 | pass | pass | pass |
| Empty result with native projection | pass | pass | pass |
| OPTIONAL term bag and absent binding | pass | pass | pass |
| False native ASK boolean | pass | pass | pass |

The nine-term fixture distinguishes IRI from identical string text, language, numeric datatype and lexical form, empty text, boolean, infinity and decimal. Native QuerySolutions variables and RDF term fields supply the answers; expected metadata is not inserted into the producer. In particular, `"01"^^xsd:int` and `"1"^^xsd:integer` remain distinct. An independent host consumer reconstructs the expected bags from the frozen Jena reference rather than trusting the worker's verdict flags. No blank nodes appear, and no general SPARQL conformance follows.

The three modes execute in distinct Python child processes in one fresh container. Disk-load creates and loads one new SQLite file; disk-reopen requires that existing file and performs no load. Process exit supplies the connection-close boundary; no undocumented close API is invoked. The disk file is on a bind mount served from macOS through virtiofs, not a native Linux host filesystem. This attempt records no locking or I/O errors, but does not qualify other filesystems. The final file is 118,784 logical bytes for nine quads; schema/index page overhead makes that tiny size unsuitable for predicting a 13.4M-triple import. It is not a full-store footprint estimate.

The actual runtime was CPython 3.11.18 on Linux aarch64, pip 24.0, Oxilite 0.10.0. The cp39-abi3 manylinux_2_28_aarch64 wheel was 9,339,632 bytes, SHA256 `bb23ced2a151c39d8c11bb71c135782189ab3dbdea7ab36bfb0a87e352b27a82`. Only that hash-verified wheel was installed offline with no dependency resolution or bytecode compilation. Eighteen installed files and eighteen RECORD entries were verified; the interpreter hash and installed-file hashes were checked again before each profile and after all profiles. The pinned image digest and actual image ID, interpreter and installer versions are retained. The bundled SQLite library version is unobserved; a host Python sqlite3 version is not a substitute.

Setup and queries are separate reviewed actions. Setup recorded runtime manifest SHA256 `47898146208f4eccb09fc618e9fef38bd4088cbb83898037f133f835fa14c89c` without importing Oxilite or querying it. Its immutable engine-import/query flags describe setup time; the later query records document actual execution. The prospective query record SHA256 `a2484fe472a3e8846aa41953cfbd5e3044268394aaf3601b6b19f27e7da44792` binds that runtime, the setup receipt and source freeze `a3481b70e1d30d49093ec9920c42f78a2656127a2cfe5967efcc44f3bb304c0d`. One query attempt ran after separate runtime/execution review, without retry or query changes.

The inspected container settings were Linux ARM64, 4 CPUs, 12 GiB memory and total swap allowance, network none, a read-only root, private cgroup namespace, bounded 64 MiB tmpfs and four Rayon threads. Source, lock and installed query runtime were read-only mounts; only the declared output mount was writable. Setup had a 180-second work cap, queries a 120-second work cap; cleanup continues after a cap if necessary. Both stages exited zero without timeout or OOM, and source/runtime checks passed. Their exact containers were removed, attach processes reaped and the owned machine lock released after exit. No memory-performance or cache-control claim is made.

All 24 saved explain headers state “fully compiled to SQL.” Full text is retained and the separately recorded interpretation is restricted to a compiler diagnostic; this does not trace runtime dispatch or prove that no fallback evaluator ran. The raw worker records keep their route unclassified.

The 65-file packet includes frozen source, pins, fixtures, setup/runtime manifests, prospective records, native answers, explain text, inspections, sampled guards and cleanup/postflight receipts. It excludes the installed wheel/binaries, container image and generated SQLite database. Portable replay verifies every packet/source hash and recomputes all 24 native-answer verdicts without importing an engine or executing archived code. It checks the retained installation identities but cannot reconstruct or rehash omitted binaries. This result permits proposing a separately reviewed, bounded workload qualification; it does not establish a timing baseline or close the wider inventory.
