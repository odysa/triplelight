# Dictionary block cache: reference-bit eviction with 1-in-4 admission (task #34, Phase B2)

**Change.** The disk dictionary block cache used FIFO eviction. A hit didn't reorder anything, so a query that
re-reads a working set slightly larger than a shard missed on every access (A2). Now a hit sets a reference bit.
A miss into a full shard is admitted with probability 1/4. To make room, the cache samples 8 residents and evicts
one that was unreferenced, clearing the bits it passes and never evicting a block whose bit that same sweep
cleared. Each shard has its own fixed-seed xorshift generator. The budget and the charged block sizes are
unchanged.

**Qualification: deterministic evidence, agreed in advance** (DESIGN.md, Amendment 7; Kaiming #34 33733518,
Jianlin 2da7c979, approvals ac091284 and 69319bc9).

| Evidence | Result |
|---|---|
| Step 1 probe (A2 cells, occupancy) | 39.6 KB charged per 256-row block; Q9 needs 2.148 GB against 2.10 GB at 1.5×; no shard skew |
| a38: A2 sweep re-run on the new policy, 30 phases, audited by Jianlin | slot-7 hit rate Q9 1× 0 → 0.62, Q9 1.5× 0 → 0.98, Q14 1× 0 → 0.67; lookups identical; anon within 1.5%; Q9 1× decode −60.5% |
| BI10M q3 trace counters, old vs new | identical, all hits |
| Hit-path microbenchmark (`b2_hit_path`) | old 25.63 ns, new 24.58 ns per hit |
| Miss-path microbenchmark (`b2_miss_path`), full shard | old 88.3 ns, new 113.0 ns per miss (+24.7 ns; frozen ceiling 9,000 ns) |
| Gate D: D1, 90 requests cold, and D1b, both full schedules warm | PASS: identical answers, lookups and misses. At the default budget no shard ever fills (max 17.5%), and all full-shard policy counters are 0, so **D1/D1b don't exercise the policy**; a38 is the evidence for full caches |

**End-to-end timing on this host is unresolved**, and every attempt is kept:

| Gate | Outcome |
|---|---|
| v1 wall, REPS=2 | no decision (contended); BI −1.65%, LUBM −3.4% |
| v2 wall, REPS=4 | no decision (contended); BI −5.1%, LUBM −12.9% |
| v3 wall, REPS=4, in-place replacement | **FAIL**: BI q3 +11.9% (+68 ms), BI total +5.3%; replacements old 8 / new 2 |
| v4 wall + host sampler, REPS=6 | **no decision**: slot 11 exhausted replacements (Docker sampler 5/31, host 2/73) |
| CPU-time A/A null run | **FAIL**: q3 0.969 and q6 1.027 against itself; the CPU A/B was therefore not run |

The q3 counters and the hit-path microbenchmark rule out a per-hit or miss-count cause for v3's q3 result. In this
run, q3 varies by about ±3% against itself. Files: the frozen design with all amendments (`DESIGN.md`), the
simulators, and the gate outputs (`*.json`, `gate-v4-replacements.log`).
