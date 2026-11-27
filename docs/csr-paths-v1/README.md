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

## Follow-up: building at the first closure, and a shared adjacency (#147, 2026-11-27)

Main `35e63d31`, LUBM-1000 four-order store, `--cache-mb 6144`, a fresh server per arm after "statistics ready", arms run
twice in reversed order, first request and median of 3 warm repeats per start; SHA-256-identical answers in every arm.
Unscored laptop evidence; protocol frozen before the run (`notes/path147.sh` in WindowsXP's workspace, sha256 `d61b26b1…`).

| Query | default warm | eager warm (CSR at the first closure) | CSR off warm |
| --- | ---: | ---: | ---: |
| `?s memberOf ?d . ?d p+ ?u` | 1,872 ms | 1,846 ms (0.986×) | 1,800 ms |
| `<group0> p+ ?y` (one bound start) | 0.4 ms | 20.2 ms | 0.4 ms |
| `?x p+ ?y` | 46.4 ms | 46.0 ms | 220.9 ms |

- **The lazy build threshold (`edges / 32` seeks) is right for point lookups:** building at the first closure makes a
  one-node lookup 50× slower (0.4 → 20 ms).
- **The joined query's ~1.8 s is not the closure:** building the CSR at once, or not at all, changes it by under 4%. Its
  time is the join of about 1M `memberOf` rows with the closure output. A shared or stored adjacency (#147) therefore
  has no warm ceiling on these queries, and step 1 was dropped by its stop rule.
