# First-column keys from the block directory, block-parallel (task #105)

After #234 the zero-length domain of `?x p* ?y` (`graph_nodes()`) was still a full SPO scan plus a full OSP scan,
read through the query iterator and bounded by decoding every block on one thread each: about 4.5 s of the 7.4 s on
LUBM-1000.

The change:
- `Snapshot::first_keys(perm)`: the distinct values of `perm`'s first key column, ascending. The default is one full
  scan (`first_keys_by_scan`).
- `DiskIndex::first_keys`: reads, verifies (SHA-256 and ordering, as a scan does) and decodes every block in parallel,
  without populating the shared block cache, and keeps each block's distinct first-column values. Blocks are in key
  order, so only a block's first value can repeat the previous block's last.
- `DiskSnapshot` answers it from the always-kept order with the same first column (SPO, PSO or OSP), so SOP and OPS
  need no regrouping. `Overlay` passes it to the base when its delta is empty and scans otherwise.
- `graph_nodes()` asks for SPO and OSP first keys side by side and merges them as before.

Laptop A/B on the LUBM-1000 four-order store, main `bc0369a9` against the branch, the same protocol as
`docs/path-graph-nodes-v1/` (blocks main, branch, branch, main). The stdout SHA-256 was identical across all runs
and both arms for every query, including the three `LIMIT 300000` slices of the unordered `p*` output.

| Query | main | branch | ratio |
| --- | ---: | ---: | ---: |
| `COUNT(*) { ?x p* ?y }` | 6.92 s | 3.34 s | **0.48** |
| `?x p* ?y` slice at 0 | 7.74 s | 4.12 s | 0.53 |
| slice at 21.7M | 7.73 s | 4.14 s | 0.54 |
| slice at 33.3M | 7.69 s | 4.13 s | 0.54 |
| `COUNT(*) { ?x p+ ?y }` | 59.3 ms | 60.8 ms | 1.03 |
| `COUNT(*) { ?u ^p+ ?x }` | 63.6 ms | 60.3 ms | 0.95 |
| `<group0> p+ ?y` | 0.7 ms | 0.7 ms | 1.02 |

Together with #234, `COUNT(*) { ?x p* ?y }` went from 13.5 s to 3.3 s (0.25×). What's left is mostly building the
33.5M result rows.

Tests: `first_keys` against a scan across block boundaries and on a corrupted block (`disk_index`), against a scan for
all six orders on six-order and four-order bases (`derived_orders_answer_like_stored_ones`), and through an overlay
whose delta deletes a subject's only triple and adds a new subject (`store`).

`ab.py` reproduces the run (set `LUBM1000_STORE`, `TL_MAIN`, `TL_CAND`). Laptop mechanism evidence only.
