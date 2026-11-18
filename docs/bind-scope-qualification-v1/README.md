# Bind scope and BASE qualification

`311cc51d978626b9a3bb860f420983d6bbc3819b` passes 25 independently evaluated Jena 6.2 controls and all 60 retained BI100M queries. Every BI result passes the frozen primary, COUNT-compatible and value-compatible gates, and matches the prior baseline's complete RDF-term bag and sequence. This is correctness qualification, not a performance comparison or general SPARQL conformance claim.

## Retained attempts

| Pin | Outcome | Subsequent action |
| --- | --- | --- |
| 9f106f1 | Clean locked release build fails E0433: oxiri used without a direct dependency. No queries run. | Author added the dependency and lock entry. |
| 8daff86 | Four of six Jena controls fail: UNDEF in BIND/FILTER/OPTIONAL and a possibly bound UNION variable. All four failures also reproduce on ef7a778. | Author replaced may-bound analysis with certainly-bound analysis. |
| 4e54448 | All six retained controls pass; new constant BIND target conflict fails (`VALUES ?x {1} { BIND(2 AS ?x) }`). Jena returns no rows, candidate returns 2; prior baseline also fails. | Author added Extend compatibility checks. |
| 311cc51 | All 25 live Jena controls and all 60 BI queries pass. | Qualified for the scoped merge. |

The final controls cover retained failures, equal/conflicting/error-producing BIND targets, datatype/lexical/language term identity, intentional EXISTS correlation, subquery LIMIT/OFFSET and BASE-relative IRI/URI resolution. The independent Jena CLI uses a 512 MiB heap and one active processor on a tiny fixture. The BI run uses the existing immutable 100M snapshot and frozen references/policies; it is a native correctness pass. Startup and query measurements in raw logs are not comparative timing evidence.

Each evidence directory retains source identities, build logs, commands and raw results. Executables and Python bytecode are omitted; binary hashes remain. The final postcheck verifies source, executable and snapshot identity, all result gates and exited owned processes. Earlier unsuccessful attempts remain visible. Kaiming reports the qualified pin merged as 99ad969 (only README differs), with the W3C results updated separately to 280 pass and 7 fail. Those W3C counts are author evidence, not a suite rerun by this qualification.

An earlier author full-suite run reported `exclusive_open_and_recovery` failing with `Locked`, while the isolated test passed. A later clean suite passed. Neither result proves the earlier failure was a pre-existing flake; that history is retained without that attribution. Alex owns the separate reproduction investigation.

`SHA256SUMS.json` hashes every report/evidence file except itself. Evidence root names preserve the actual tested pins. Original runtime paths in scripts are historical provenance and must be adapted explicitly before replay.
