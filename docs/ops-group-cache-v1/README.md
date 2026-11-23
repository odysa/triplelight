# Regrouped-group cache for derived orders (#105, storage lane)

**The problem:** a one-column scan on a derived order (OPS with the object bound and the predicate a variable, or SOP
with the subject bound) regroups the key's whole sibling group: OSP for OPS, SPO for SOP. It sorts the group before
the first batch comes out. `scan_bench` measured about 21 ms per OPS probe on LUBM-1000, because class objects have
millions of rows.

**The change:** the per-snapshot memo also keeps the last few regrouped groups, at most 8 and at most `spill_rows()`
rows in total. `spill_rows()` = max(1M rows, a quarter of the block-cache budget), the in-memory sort limit. A
one-column derived scan reuses a cached group. A miss whose group fits the limit is sorted in memory and cached. A
larger group regroups and spills as before. A one-column seek that misses its value memo takes the distinct values
from a cached group. The memory is outside the block-cache budget, bounded as above.

**Setup:**
- Laptop, one session, ABBA: main (`binaries.txt`: main at `14f381d0`, the branch at `cfef9c07`).
- `scan_bench --keys 5000 --passes 1` on a four-order LUBM-1000 store (133,613,894 triples). Raw output is in
  `r*.jsonl`.
- The row checksums are identical in all four runs for every phase. Main against main is within 3% for these phases.

| Phase | main (s) | cache (s) | cache / main |
| --- | ---: | ---: | ---: |
| `repeat1_Ops`: 50 hot objects, 4 at a time, 25 rounds | 43.9 / 42.6 | 0.58 / 0.59 | 0.014 |
| `prefix1_Ops`: first batch on 5,000 sampled objects | 107.5 / 104.5 | 36.5 / 36.7 | 0.35 |
| `seek_Ops`: rising one-column seeks on the same keys | 63.6 / 62.8 | 36.8 / 36.7 | 0.58 |
| `prefix1_Sop` | 0.038 / 0.038 | 0.031 / 0.031 | 0.82 |
| `repeat1_Sop` | 0.003 / 0.003 | 0.000 / 0.000 | 0.1 |
| Full scans and kept-order phases | | | 0.99–1.01 |

What the table shows:
- **Sampled probes still gain:** the 5,000 keys come from evenly spaced SPO rows, so common objects (classes) repeat.
- **The remaining cost** is the first regroup of each distinct large group. The built-on-demand order of
  `docs/adaptive-orders-design.md` removes that.

`widths.jsonl` is the `block_widths` census of the same store (see #105), recorded here because it ran in the same
slot.
