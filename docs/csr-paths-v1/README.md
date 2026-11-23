# CSR adjacency for property-path closures: laptop A/B (task #105)

The LUBM-1000 four-order store, one release binary. The `off` arm is `--ablate=csrpaths`. Blocks ran in the order on,
off, off, on. Each block ran a 25-second statistics warm-up, then each query once untimed and three times timed. The
table shows the median per arm. Response bytes were SHA-256-identical across every run and both arms. Laptop mechanism
evidence only.

| Query (`ub:subOrganizationOf`) | CSR on | off | ratio |
| --- | ---: | ---: | ---: |
| `COUNT(*) { ?x p+ ?y }` | 53.5 ms | 257.5 ms | **0.21** |
| `COUNT(*) { ?u ^p+ ?x }` | 51.3 ms | 264.3 ms | **0.19** |
| `COUNT(*) { ?x p* ?y }` | 12.3 s | 21.0 s | **0.59** |
| `<group0> p+ ?y` (bound start) | 0.4 ms | 0.4 ms | 1.1 (below the build threshold) |
| `?s memberOf ?d . ?d p+ ?u` (many bound starts) | 2.22 s | 2.17 s | 1.02 |

The joined query stays on per-node seeks. Each bound start reaches only a few nodes, so the lazy build threshold
(`edges/32` seeks) is not reached in that query. `p*` with both ends unbound is dominated by the zero-length
pairs over every graph node (`graph_nodes()`), which CSR doesn't change. A follow-up is SCC condensation, plus a
cheaper path for the zero-length part.

`ab.py` reproduces the run (set `TRIPLELIGHT_BIN` and `LUBM1000_STORE`).
