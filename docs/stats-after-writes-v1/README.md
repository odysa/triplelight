# Query speed after a write (#111)

The release checklist's gate is that, after a committed update and before compaction, each registered analytical query
stays within 1.20× of its time before the update. On main `a10bf73d` that gate failed. One INSERT of a triple no query
reads made BSBM BI q1 3.3× slower, q3 1.24×, q5 1.37× and q2 1.18×, with the same answers. Two causes were stacked.

1. **The overlay's direct paths turned off for the whole store.** Any non-empty delta sent every scan, seek,
   `scan_from` and borrowed row view through the merge, even for ranges the delta doesn't touch. `scan_from` was the
   worst case: it fell back to a full scan and a skip.
   **Fix (this PR):** `Delta::untouched(perm, prefix)` does two binary searches in the sorted delta orders. A range with
   no adds and no dels takes the base's own `scan`, `seek`, `scan_from`, `matching_rows` and `matching_tail`. A prefix
   holding an ID first seen in the delta is exact, because rows with that ID exist only as adds. That case, an unused ID
   and the borrowed views are all covered by `untouched_ranges_answer_like_the_merged_view`. The test checks every order
   and every prefix of length 0 to 2 against the full merged scan.
2. **The planner dropped its statistics after any write.** That's the companion PR. Statistics plan while the delta is
   at most a tenth of the base.

**Laptop ABBA**, on a BSBM-10 `serve-store` copied fresh for every block, with the 15 m000 BI queries. Each arm warms
twice and times 10 runs, using in-process HTTP to the last byte, and answers were identical in every run. The five arms
run in the order pre, post, stats, range, both, both, range, stats, post, pre:
- `pre`: main, no update;
- `post`: main after one INSERT;
- `stats`: the statistics change alone, after the INSERT;
- `range`: this change alone, after the INSERT;
- `both`: the two changes together, after the INSERT.

| Query | pre | post | stats | range | both | post / pre | both / pre |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| q1 (p00) | 6.05 | 19.98 | 8.46 | 16.01 | 6.02 | 3.30 | **1.00** |
| q3 (p01) | 78.40 | 96.93 | 94.46 | 83.93 | 85.47 | 1.24 | **1.09** |
| q2 (p02) | 2.18 | 2.40 | 2.28 | 2.16 | 2.17 | 1.10 | 1.00 |
| q2 (p03) | 1.10 | 1.30 | 1.31 | 1.11 | 1.11 | 1.18 | 1.01 |
| q5 (p05) | 4.00 | 5.48 | 5.38 | 3.98 | 4.06 | 1.37 | 1.01 |
| q7 (p11) | 53.76 | 58.09 | 57.32 | 52.65 | 51.19 | 1.08 | 0.95 |
| q8 (p12) | 1102.5 | 1122.9 | 1105.8 | 1087.2 | 1088.3 | 1.02 | 0.99 |

Times are in ms. The other 8 queries run under 0.5 ms on every arm, within noise. With both changes, every query is
within 1.09× of its pre-update time, so the gate passes on this laptop. Neither change is enough alone: the range
check fixes q3, q5 and q2, and the statistics fix q1. Full rows are in `runs-bi.txt`.

For LUBM-100 and agg-WatDiv, with the statistics change only, `runs-lubm100.txt` and `runs-agg-watdiv.txt` show no
post-update gap on today's main. Since #243, the star counts don't depend on the planner. These are laptop numbers;
the rental runs repeat them.
