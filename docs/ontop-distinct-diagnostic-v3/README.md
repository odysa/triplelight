# Ontop COUNT(DISTINCT) lexical-collision diagnostic

Ontop 5.5.0 with DuckDB JDBC 1.5.6.0 returned 8 distinct terms in the original aggregate, while its DISTINCT projection returned all 9. This follow-up isolates that discrepancy on the same R2RML mapping and Java 25.0.5.1+2 runtime on macOS ARM64.

| Fixture | COUNT(DISTINCT) | RDF reference |
| --- | ---: | ---: |
| Original | 8 | 9 |
| Change the string spelling only | 9 | 9 |
| Change the integer value only | 8 | 9 |
| Apply both changes | 9 | 9 |

All four fixtures pass plain projection, DISTINCT projection, term GROUP BY and COUNT. The SQL returned by Ontop's reformulation API is byte-identical across the four fixtures. Its aggregate counts distinct values of the lexical-string column alone. The DISTINCT projection also carries an RDF type tag used to reconstruct the terms. The original IRI and string share a lexical spelling, so they collapse in the aggregate. The two integer terms have different spellings and stay distinct in this diagnostic. This conclusion applies to the pinned mapping and fixture.

The first attempt compiled and then hit its 32 MiB per-file limit while JDBC unpacked the native library. No fixture or query ran in that attempt. The repaired attempt verified the 107,640,704-byte library against the pinned jar in every process and completed all four fixtures. A 110 MiB per-file limit accommodated the library within the 256 MiB attempt cap. Both attempt receipts are included.

## Workload screen

The frozen workload contains 72 aggregate labels and 60 BI requests, including 15 BI warmups. Thirty-six aggregate labels use COUNT(DISTINCT). In 35, the counted variable occurs in subject or predicate position, so the illustrated IRI–literal collision cannot occur on that variable. The remaining label, WatDiv `distinct_L2.rq`, counts an object-only variable bound through `parentCountry` and `nationality`. Keep it out of any provisional Ontop aggregate cohort until the mapped term domain and the exact answer are verified. This screen does not establish that those predicates actually mix IRIs and literals. None of the 60 BI requests uses COUNT(DISTINCT). Any remaining queries would need same-mapping answer qualification against their frozen references before timing.

The packet contains the typed answers, intermediate and native reformulations, fixture inputs, resource receipts, and static workload screen. It contains no workload timing or admission. The reported SQL comes from Ontop's reformulation API on the same repository connection as the typed queries.

Run `python3 -I replay.py` alongside the packet to verify all 77 archived members and replay the result without Java or DuckDB. The expected output is AUDIT.json.
