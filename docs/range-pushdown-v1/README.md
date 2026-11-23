# Range-filter pushdown: laptop A/B on BSBM-100M BI q3 (task #105)

Laptop (Apple Silicon, shared with other workloads), one TripleLight server per block, the same release binary in both
arms. The "off" arm is the same binary with `--ablate=range`. Six blocks in the order on, off, off, on, on, off. In each
block, every q3 instance (the four frozen BI requests `m00{0..3}_p01_q3`) gets one warm-up and then three timed runs over
HTTP. The table shows the median over all timed runs per arm. The response bytes were SHA-256-identical across all
runs and both arms in every version. Mechanism evidence only, not a benchmark claim.

| q3 instance | v1 on / off (ms) | v2 on / off (ms) | v3 on / off (ms) | v3 ratio |
| --- | --- | --- | --- | ---: |
| m000 (2007 window, sparse) | 212 / 614 | 212 / 613 | 206 / 612 | 0.34 |
| m001 | 1918 / 1093 | 866 / 1095 | 857 / 1080 | 0.79 |
| m002 (densest) | 1581 / 1388 | 1555 / 1416 | 1369 / 1378 | 0.99 |
| m003 | 1922 / 1194 | 1180 / 1207 | 1101 / 1189 | 0.93 |

- **v1:** the ranged pattern was read first. Its rows come out in date order, so the next join made one probe per row.
  The dense windows regressed by 1.14–1.75×.
- **v2:** the next join chooses between one probe per row and one subject-ordered scan with a hash semi-join on the
  ranged keys. The ranged read itself switches to one predicate scan when the range is dense. The densest window was
  still 1.10×.
- **v3:** a selectivity gate. A range covering more than 15% of the predicate's triples is not pushed at all, and the
  plan is then the unranged plan. Each BGP's decision shows on the server's `phase_trace` line, for example
  `range_pushdown=on:0.010,on:0.007` (m000) or `off:0.158,off:0.158` (m002), with one entry per BGP. For an `off`
  entry, the fraction is a lower bound, because counting stops once it passes the limit.

Raw per-run records: `v1-ungated.json`, `v2-semijoin.json`, `v3-gated.json`. Each record has the block, arm, query, run,
seconds and response hash.
