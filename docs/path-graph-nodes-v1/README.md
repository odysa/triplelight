# Zero-length path domain: merge instead of hash, scans side by side (task #105)

`?x p* ?y` with both ends unbound pairs every graph node with itself (`graph_nodes()`), then adds the closures. On the
LUBM-1000 four-order store, `COUNT(*) { ?x ub:subOrganizationOf* ?y }` spent its time there, not in the CSR closure:
a profile showed the SPO and OSP full scans plus a SipHash set over 37.9M keys (21.7M subjects, 16.2M objects),
both single-threaded. The closures themselves took 0.78 s.

The change (`crates/triplelight-exec/src/path.rs`):
- Both scans are sorted by their first column, so the objects that are not subjects are found by a merge against the
  sorted subjects. The output order is the old one (subjects ascending, then the remaining objects ascending). A view
  whose scan is not sorted, checked as the keys arrive, keeps the hash set.
- The two scans run side by side (`rayon::join`) on the same immutable view.

Laptop A/B, one release binary per arm (main `dc6ef71a` and the branch), CLI `query --disk`, blocks main, branch,
branch, main. `star`, `plus`, `rplus` and `bound` ran 1 untimed and 3 timed per block; the three `ord` slices ran once
per block. The table shows medians. The stdout SHA-256 was identical across all runs and both arms for every query,
including three `LIMIT 300000` slices of the unordered `p*` output (start, the subjects/objects boundary, the end),
so the row order is unchanged.

| Query | main | branch | ratio |
| --- | ---: | ---: | ---: |
| `COUNT(*) { ?x p* ?y }` | 13.51 s | 7.42 s | **0.55** |
| `?x p* ?y` slice at 0 | 14.47 s | 8.15 s | 0.56 |
| slice at 21.7M | 14.33 s | 8.15 s | 0.57 |
| slice at 33.3M | 14.43 s | 8.11 s | 0.56 |
| `COUNT(*) { ?x p+ ?y }` | 63.1 ms | 61.8 ms | 0.98 |
| `COUNT(*) { ?u ^p+ ?x }` | 64.2 ms | 63.5 ms | 0.99 |
| `<group0> p+ ?y` | 0.8 ms | 0.8 ms | 1.03 |

`p`, `+` and bound starts don't call `graph_nodes`; they are controls. What is left of `p*` is the SPO scan (about
4.5 s, bounded by decoding every block) and building 33.5M rows. A scan that decodes only the first column, or splits
blocks across threads, is the next step and would be a storage-read change of its own.

`ab.py` reproduces the run (set `LUBM1000_STORE`, `TL_MAIN`, `TL_CAND`). Laptop mechanism evidence only.
