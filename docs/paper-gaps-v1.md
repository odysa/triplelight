# Pre-submission gap audit: VLDB 2027 systems paper (task #100, 2026-11-22)

Sources: Kaiming's hostile-reviewer read of draft PR #150 and `docs/eval-plan/PLAN.md`, and the area audits in #100
(Alex: storage; Jianlin: correctness and analytics; WindowsXP: servers and writing). Status codes:
**B** = blocks submission, **W** = weakens the paper (reviewers will hammer it), **m** = minor. "Before rental" means it
must be done before the dedicated-machine window, so the window measures the final configuration once.

## B: blocks submission

| # | Gap: what a reviewer says | Fix | Owner | When |
|---|---|---|---|---|
| B1 | **No results.** C1a and C1b are empty: "SOTA" is a claim with no data. | The registered rental run (S1–S4, C1a, C1b). | all | rental |
| B2 | **Direct competitors missing.** "RDF has a similar gap" is false as written. The DuckDB `rdf` community extension already runs SPARQL inside DuckDB, RDF Fusion (IEEE Access 2025) runs SPARQL on Arrow/DataFusion, and MillenniumDB is a disk-based worst-case-optimal SPARQL engine. | Add the DuckDB `rdf` extension and RDF Fusion to C1a, and MillenniumDB to C1b, each answer-gated. Rewrite §1's gap statement against them. | WindowsXP (servers, MillenniumDB), Jianlin (the two embedded ones) | before rental |
| B3 | **Synthetic data only** (Duan et al. 2011, our own §5). | Add **WDBench** (Wikidata truthy plus real queries from the Wikidata logs, ISWC 2022) as the real-data workload. It also gives #97 its cyclic queries and #98 real derived-order coverage. | Jianlin (workload, answers), Alex (load at about 1.2B triples) | before rental: parser test on a truthy prefix only; rental: 30-request answer gate and full run (plan A3) |
| B4 | **No 1B result, no cold cache, no memory-limited runs** (Alex B1–B3). | S1–S3 on the rental. Build the first 1B store first, since it's the biggest risk. Re-run the #83 memory sweep on main after #131 now (1 laptop hour). | Alex | sweep now; rest rental |
| B5 | **The ranking test can't run as registered:** there's no RDF4J script and no QLever two-order arm (WindowsXP A1, A2). | Add both to `bench/eval/systems.sh` with receipts of the created indexes. | WindowsXP | before rental |
| B6 | **Baselines not tuned or documented** (WindowsXP A3): no `oxigraph optimize`, no Jena `tdbstats`, QLever and Virtuoso settings undocumented. That's a classic rejection reason. | Apply each vendor's documented tuning and record it in the receipts. | WindowsXP | before rental |
| B7 | **Answer coverage and policy not published per cell** (Jianlin 1, 2). | A coverage table per scale and API: scheduled, admitted, wrong, unsupported, timeout. Negative controls, and answers frozen before candidate output at each scale. | Jianlin | before rental: table and controls; rental: fill |
| B8 | **Replication unit and statistics not pinned** (Jianlin 6). | Independent fresh-process blocks, paired and counterbalanced, medians and IQR plus paired ratios, timeouts kept separate, a margin registered for "unchanged". Plan amendment plus driver change. | Jianlin (amendment), WindowsXP (driver) | before rental |
| B9 | **No insight beyond "fast".** Research-track reviewers want to know *why* it's fast. | Add a time breakdown per query class (parse, plan, join, decode, serialize) and decode-deferral counts, TripleLight against the fastest rival. One small instrumented build; registered as S6. | Kaiming | before rental: instrument; rental: measure |
| B10 | **Related work unread** (Hexastore, the Ring, Kùzu, Veldhuizen, Neumann–Moerkotte, DuckDB 2019, MillenniumDB, Hogan et al., Abadi et al., LUBM/BSBM/WatDiv, plus Tentris, WDBench, gStore, the DuckDB `rdf` extension, RDF Fusion). | Read them and drop every †. About 1.5 days. | WindowsXP | now |
| B11 | **SPARQL coverage overclaimed.** The abstract says "SPARQL 1.1"; the current run is 323 pass, 7 fail (cast/functions lexical forms, `seconds`, `tz`), 1 unsupported, and 22 skipped of 353. The archived README still shows 281. The harness doesn't check order or blank-node identity, and the named-graph skips persist although named graphs are supported. | Fresh per-case inventory on the paper binary; un-skip the named-graph tests; add order and blank-node checks; state "a SPARQL 1.1 subset" with an explicit limits list. Report Update separately. | Jianlin (harness), Kaiming (README refresh) | now |
| B12 | **Dual submission.** #145 (E&A), the demo and #150 overlap. | Decided: only #150 goes to the research track. The demo has its own text and scenario and is submitted only if VLDB's demo rules allow it alongside. #145 is not submitted while #150 is under review. | Kaiming (decision), WindowsXP (text) | decided |

## W: weakens the paper

| # | Gap | Fix | Owner |
|---|---|---|---|
| W1 | **DuckDB arms look unfair** (Jianlin 4): hand-written SQL on one table. | Finish the three arms plus a single sorted-SPO control. Disclose that the SQL is hand-written, and use the DuckDB `rdf` extension (B2) as the automatic translation. | Jianlin |
| W2 | **Memory budget rule isn't uniform** (WindowsXP A4). | Decided: one rule. Every system gets the same cgroup `memory.max`; inside it, each system's memory knob follows its vendor's guidance, stated per system in §4.1. TripleLight follows the same pattern (half the limit). | WindowsXP |
| W3 | **Docker for QLever and Virtuoso, native for the rest** (A5). | On the Linux host, a control run showing that host networking and a local volume make no difference, or run them natively. | WindowsXP |
| W4 | **Timeouts leave the server running** (A6). | `restart.sh` kills and restarts after each timeout; test it in the rerun. | WindowsXP |
| W5 | **Laptop timings in the draft** (§2 stats-ready time, §3.4 build profile, §3.5 cache sweep). | Mark them "laptop, preliminary" now, and replace them with rental numbers. | WindowsXP |
| W6 | **Durability claims rest on unit tests only** (Alex W1). | Scope the claim to "WAL with fsync; crash states between publish steps are tested". No fault-injection claim. | Alex (text) |
| W7 | **Updates and compaction are unmeasured** (Alex W2). | Scope the paper as read-mostly analytics; S5 optional. | Alex |
| W8 | **Concurrency and parallelism:** one process, one writer. Is intra-query execution parallel? | State both in §2 and the limits; measure thread scaling on one suite if it is parallel. | Kaiming |
| W9 | **Derived orders measured only where they're cheap** (Alex W4). | Forced-plan stress test plus WDBench coverage (B3). | Alex |
| W10 | **Workload representativeness** (Jianlin 5). | Per-template results, selectivity, empty rate and distinct-text sensitivity; name each graph by hash. | Jianlin |
| W11 | **Commercial engines absent** (GraphDB, Stardog, AllegroGraph). | One sentence: licence terms on publishing benchmarks aren't accepted on the owner's behalf, so they're excluded. | WindowsXP |
| W12 | **Kùzu:** archived upstream, and it uses Cypher. | Related work only, with the reason. | WindowsXP |

## m: minor

- Switch to the PVLDB Vol. 20 template and check the 12 pages. Add figures: architecture, build pipeline,
  derived-order path. (WindowsXP)
- Sources for the general statements in §1 and for "QLever deduplicates before sorting" (file and commit). (WindowsXP)
- Evidence map from claim to file, as a repo doc. (WindowsXP)
- A format appendix pointing to `docs/persistence.md`, plus "no downgrade path". (Alex)
- The cgroup v1 fallback (`storage/cgroup-v1-root`). (Alex)

## Schedule impact

The before-rental items are desk work plus about 3 qualification slots, spread across four people. That's roughly
1–1.5 weeks, so the rental starts mid-December as planned. The rental itself takes 1–2 weeks with the WDBench and
1B builds. **February 1 is tight but feasible; March 1 is the realistic target, with February 1 as a stretch.**
