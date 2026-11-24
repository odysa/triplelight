# Single-key COUNT over the flat BGP buffer (#105)

Two changes to `GROUP BY ?k` with one `COUNT(*)` or `COUNT(?v)` over a BGP (or a FILTER pushed entirely into it):

1. `agg/flat.rs`: count the BGP's flat `Vec<TermId>` in place instead of building a `Row` per solution.
2. `count_one_key`: intern each distinct count value once instead of once per group.

## ABBA, BSBM-100M, server mode, laptop

Main `5c74061d` (A) against this branch (B), sessions A B B A, one warm pass then 5 timed passes per session,
medians over the 10 timed runs per arm. All answers byte-identical across arms.

| query | A (ms) | B (ms) | B/A |
|---|---:|---:|---:|
| BI q3 m000 | 119.4 | 112.8 | 0.945 |
| BI q3 m001 | 479.3 | 464.1 | 0.968 |
| BI q3 m002 | 632.0 | 615.3 | 0.974 |
| BI q3 m003 | 568.0 | 546.6 | 0.962 |
| BI q2 m000 | 42.0 | 41.7 | 0.992 |
| BI q2 m002 | 53.7 | 53.5 | 0.996 |
| BI q1 m000 (control) | 55.5 | 54.5 | 0.982 |
| BI q7 m002 (control) | 23.2 | 23.2 | 1.001 |

q3 geomean 0.962. Change 1 alone measured 0.98-0.99 on q3; the rest is change 2.
Phase trace, q3 m002: `group_us` 145 ms (main) to 127 ms (branch); `bgp_us` unchanged.
