# Bounded regroup for derived orders: sizing and LUBM-1000 measurements (PR #187)

Plan amendment 1, A8 / gap W9. On a four-order store, SOP and OPS requests with zero or one bound column regroup the
stored sibling order one first-column group at a time (`crates/triplelight-storage/src/derived_order.rs`). Before PR #187
a group was held whole in memory; since #187 a group above max(1M rows, a quarter of the block-cache budget) is spilled
through the fixed-width external sorter into `<store>/scratch/`, with identical output. Laptop evidence (macOS arm64,
10 cores, 64 GiB); mechanism only, not timing claims for the paper's comparison.

## Sizing: the largest object groups (`opsgroups-*.txt`)

Object frequencies counted from the N-Triples files with one `awk` pass (raw lines, duplicates included, so slightly
high). The OPS regroup holds one object's group.

| Dataset | Lines | Largest object group | Next |
| --- | --- | --- | --- |
| LUBM-1000 | 138,318,414 | 11,168,009 (`"xxx-xxx-xxxx"`, the telephone placeholder, 8.1%) | `ub:Publication` 8,063,361; `ub:UndergraduateStudent` 7,924,765 |
| BSBM-100M | 98,861,444 | 5,696,520 (`bsbm:Offer`, 5.8%) | `rev:Review` 2,848,260 |

At 24 bytes a row in memory, linear projection gives about 89M rows (2.1 GB, up to 4.3 GB with `Vec` growth) at
LUBM-8000 and about 57M rows (1.4 to 2.7 GB) at BSBM-1B: a quarter to a half of the 8 GB S1 limit, which is why #187
bounds it.

## LUBM-1000 measurements

One LUBM-1000 four-order store (3.0 GB), built once; the `derived_stress` example (PR #186) run on it with
`TRIPLELIGHT_TRACE_DERIVED=1` under `/usr/bin/time -l`, block cache 512 MB (`--cache-mb 512`).

- **Before:** main at the time (the regroup without a bound, with `pending.drain(..4096)` from the front of one group),
  example from branch `storage/derived-stress` (573eaa61). File `before-main-full-scans.err`.
- **After:** PR #187 at ce46f0d0, its first reviewed head (the later review changes moved the spill directory and made
  the seek memo distinct-only; neither touches these paths' time or the regroup's memory). Without spilling:
  `TRIPLELIGHT_REGROUP_SPILL_ROWS=100000000000`; with the default threshold, groups above 5.6M rows spill at this budget.

| Run | Full SOP scan | Full OPS scan | Peak RSS | Spills |
| --- | --- | --- | --- | --- |
| Before (main), full scans only | 9.9 s | **21.8 s** | 0.93 GB | — |
| After, no spill, full scans only | 9.8 s | **5.1 s** | 0.93 GB | 0 |
| After, spill, full scans only | 9.8 s | 5.9 s | 0.94 GB | OPS: 3 groups (SOP: none) |
| After, no spill, top-5 prefixes + full scans | | | 1.91 GB | 0 |
| After, spill, top-5 prefixes + full scans | | | 1.68 GB | 3 groups, 27.2M rows |
| Open and the two key-ranking scans only (baseline) | | | 0.39 GB | — |

- **Time:** the full OPS scan went from 21.8 s to 5.1 s. Main's regroup appended a whole group to one buffer and took
  4096 rows at a time from its front, which is quadratic in the group size (an 11.2M-row group here); #187 reads by
  index and drops the consumed half. All runs return 133,613,894 rows for each full scan, and the example checks every
  row's prefix and order.
- **Memory:** at this scale the 512 MB cache dominates the full-scan peak, and the largest group (268 MB) is a small
  part of it, so the spill changes full-scan RSS little; with the prefix phase the spill saves about 0.23 GB. The bound
  matters where one group is gigabytes (LUBM-8000, 1B scales); the rental's four-against-six runs record
  `regroup_spills`, `spill_rows` and `peak_spill_bytes`.

## Limits

One laptop session, one run per row; peak RSS is the process maximum from `time`. The "after" numbers predate #187's
final review changes, as noted above.
