# Star BGP counts from range counts (#105)

Profile v2 had agg-WatDiv `all_C3` at 2.66× Virtuoso, all of it in the count path, and `distinct_C3` at 1.15×. C3 is a
six-pattern star on `?v0` with every other variable private. A star is acyclic, so `COUNT(*)` took the binary-join count,
which materialises every row of the first five patterns. `COUNT(DISTINCT ?v0)` was not pushed down at all: the
evaluator built every solution and then grouped them.

The change (`exec`):
- `pattern::star_center`: the one variable shared by every pattern (two or more) when every other variable occurs
  once.
- `Bgp::count_star`: the center's values are the leapfrog intersection of the patterns' ranges. Given a center value
  the patterns are independent, so `COUNT(*)` is the sum over those values of the product of each pattern's range
  count, and `COUNT(DISTINCT ?center)` is the number of values. No solution rows are built. The most selective
  pattern leads the intersection.
- `count.rs`: a star takes this path before the leapfrog and binary counts. `COUNT(DISTINCT ?x)` over several patterns
  is pushed down only when `?x` is the star's center; any other DISTINCT count still goes to the evaluator.

A two-pattern join on one variable is a star too, so it no longer scans.

**Laptop ABBA**, WatDiv-100M store, main `852d01df` against the branch: blocks main, branch, branch, main. Each block
starts its own server (`--cache-mb 8192`), warms each query twice and times it 20 times, using in-process HTTP to the
last byte. The answer SHA-256 was identical across all runs and both arms for every query. Every agg-WatDiv star is
included (they all take the new path), plus two non-star controls.

| Query | main | branch | ratio |
| --- | ---: | ---: | ---: |
| `all_C3` | 77.5 ms | 16.0 ms | **0.21** |
| `distinct_C3` | 41.1 ms | 16.3 ms | **0.40** |

The other 18 stars and both controls run between 0.2 and 1.2 ms. Their ratios range from 0.84 to 1.74, but the
`all_F5` control, whose path is unchanged, came out at 1.65. At this resolution the arms differ by about 0.1 ms
whatever path a query takes, so none of those ratios can be attributed to the change. All rows are in `runs.txt`.

Main's `all_C3` took 77 ms here against 13.3 ms in profile v2. I haven't explained the gap: the client and the cache
budget differ, and the store is the same. The ratio is what this run supports. Rival comparisons should wait for the
next profile.

Tests: `star_counts_match_the_evaluator` checks 8 star shapes against the evaluator, each with 5 counts: `COUNT(*)`,
`COUNT(?c)`, `COUNT(DISTINCT ?c)`, `COUNT(DISTINCT ?leaf)` (which falls back) and `COUNT(?leaf)`. The shapes cover a
center in object position, constants, a predicate variable, an empty pattern and a two-leaf pattern. The index-only
test now expects a star to need no scan, and a two-variable chain to still scan.

`ab.py` reproduces the run (set `WATDIV100_STORE`, `QUERIES`, `TL_MAIN`, `TL_CAND`). This is laptop evidence for the
mechanism only.
