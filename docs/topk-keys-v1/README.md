# ORDER BY … LIMIT: later sort keys only for rows that can reach the top k (task #105)

`sort_rows` computed every ORDER BY key for every row before selecting the top k. For BI q3
(`ORDER BY DESC(?ratio) ?product LIMIT 10` over 153k joined rows) that decoded all 153k product IRIs for a
tie-break that only the rows near the top can need. A row can be among the first k only if its first key is no later
than the k-th smallest first key, so the first key is computed for all rows, the k-th is selected, and the remaining
keys are computed only for rows at or before it. The final order is unchanged (same comparator, same index
tie-break). `top_k_with_secondary_keys_matches_the_full_sort` checks LIMIT and LIMIT+OFFSET against the unlimited
sort with ties, unbound keys, ascending and descending, variable and expression keys.

**CLI, BSBM-100M** (main `31cd5b96` against the branch, order main, branch, branch, main, byte-identical output):

| q3 | main (ms) | branch (ms) | ratio |
| --- | --- | --- | ---: |
| m001 | 992, 1000 | 806, 818 | 0.82 |
| m002 | 1317, 1304 | 1100, 1112 | 0.85 |
| m003 | 1156, 1142 | 962, 968 | 0.84 |

**Server ABBA** over all 186 profile-v2 queries (driver, warm): every answer class and hash is identical in all four
arms. Per-suite geomeans are in `summary.txt` (0.90–1.08). Arm 3 was noisy on queries the change cannot reach (LUBM
q6 1697 ms against 1064–1084 ms in the other three arms; LUBM q2 43 ms against 31–32 ms), which drives the LUBM
figure. BI q3 per arm (ms): m000 195/181/181/197, m001 788/798/706/782, m002 1088/1044/978/1089, m003
933/899/842/938, so 0.93–0.95× through the server, where decoding is cheaper than on the CLI (6 GB cache).
Laptop mechanism evidence only.
