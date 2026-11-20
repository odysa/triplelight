# RDFLib 7.6.0 Memory: two tiny qualification profiles

**Neither declared profile passes every tiny control.** The single untimed attempt completed normally: four fixed queries and seven checks per profile, using RDFLib Memory with default literal normalization and with literal normalization disabled (`NORMALIZE_LITERALS=False`). Both profiles and all outcomes are retained. There are no workload timings or full-workload admission.

| Check | Default normalization | Preserve lexical forms |
|---|---|---|
| Grouped projection metadata | pass | pass |
| Strict grouped RDF-term/count bag | fail | pass |
| Projected empty result for fixed FILTER(false) query | fail | fail |
| OPTIONAL projection metadata | pass | pass |
| Strict OPTIONAL object-term bag | fail | pass |
| Every missing OPTIONAL cell unbound | pass | pass |
| False native ASK | pass | pass |

Each profile stores nine triples. Default normalization rewrites two tested lexical forms: `"01"^^xsd:int` becomes `"1"^^xsd:int`, and `"INF"^^xsd:double` becomes `"inf"^^xsd:double`. The lexical-preserving profile retains both original forms and passes the two strict term-bag checks. Per-profile diagnostic receipts independently record both a constructed Literal and the parsed xsd:int object: `1`/`1` under the default, `01`/`01` with normalization False. These diagnostics do not affect the fixed seven verdict checks.

Both profiles return nine bindings, with projection `[o]`, for `SELECT ?o WHERE { <urn:s> <urn:v> ?o FILTER(false) }`, whose fixed expected result has zero rows. This is a returned-row failure; projection metadata is present. The observations do not isolate parser, algebra or evaluation as the cause. The query was not rewritten or rerun after observing failure. No conclusion about other RDFLib stores or arbitrary SPARQL queries follows.

The fixture deliberately distinguishes IRI versus string text, language, numeric datatype and lexical form, empty text, boolean, infinity and decimal. Terms and projections come from the native RDFLib API, with no expected metadata supplied to it. No blank nodes appear in the fixture. Normalization is a process-global setting; each graph is fresh, every graph is closed and the setting is restored. The two settings are separate configurations, not pooled observations or an adaptive retry.

The exact wheels were RDFLib 7.6.0 (615,416 B; SHA256 `30c0a3ebf4c0e09215f066be7246794b6492e054e782d7ac2a34c9f70a15e0dd`) and pyparsing 3.3.3 (126,420 B; SHA256 `ece8c00a69cf01b45d0b1dedabb469c90d8caf996d4fda40f147627a122849a4`). Only these hash-verified wheels were installed without dependency resolution into an isolated CPython 3.14.3 environment on macOS arm64. The 177 installed-file hashes and wheel RECORD entries were verified before query execution and the installed identities remained unchanged afterwards. Both the venv interpreter path and resolved base-binary path are recorded; the supervisor requires the announced path and binary hash before launching the child.

Approved source freeze: `5cd10e3658d00092661e43d57497a677e286f53a802ae925cde9b9e8f9307048`. Prospective execution record: `f574a1fb8f2c2f2b9a3bc92d24cb7876adeda91136b75d601e042b81d076ecdf`. One 120-second child cap covered both profiles. The child and supervisor exited 0, no timeout occurred, and all source/input/runtime identities remained unchanged. The exact owned machine lock was released after exit. No containers, VM-cache manipulation or previous-cohort reruns occurred.

The packet retains reviewed source, fixed fixtures/reference, native query answers, per-profile checks/diagnostics, runtime/install manifests and pre/post/lock receipts. Wheels, installed binaries and databases are excluded. The portable replay verifies packet/source identities and independently recomputes all fourteen verdict checks from the eight recorded answers, without importing RDFLib or executing archived code. It cannot reconstruct or rerun the engine from receipts alone. This negative admission does not close the broader embedded-store inventory or establish a performance comparison.
