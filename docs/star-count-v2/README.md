# Star counts, remeasured with planner statistics (#105)

**This corrects `docs/star-count-v1/`.** The v1 ABBA timed each server before its planner statistics were ready, so
main ran the greedy fallback join order instead of the cost-based plan that users get. Main's `all_C3` took 77 ms
there, against 13.3 ms in profile v2. `ab.py` now waits for "statistics ready", as `bench/eval/lib.sh` does.

**Measured that way, #238's star path made `all_C3` slower: 1.12× the code before #238.** `distinct_C3` stayed faster
at 0.36×. The leapfrog found the center values by seeking round-robin through every pattern, and then counted each
pattern again for its product. So each candidate paid for both a seek and a count in every pattern.

This change (`Bgp::count_star`):
- **Candidates:** the leapfrog intersection of the most selective pattern and every leafless pattern (`?c p o`).
  These are exactly the sorted lists that pin the center, and skipping ahead through them is cheap.
- **Probes:** every pattern with leaves gets one range count per candidate. A zero drops the candidate at once, and
  the patterns are probed from most to least selective.
- **Totals:** a candidate adds the product of its counts to `COUNT(*)`, and the survivors are `COUNT(DISTINCT ?center)`.

**Laptop ABBA, WatDiv-100M.** Three arms in the block order pre, main, branch, branch, main, pre:
- `pre`: `852d01df`, the code before #238;
- `main`: `3dd673cf`, #238 as merged;
- `branch`: this change.

Each server is timed only after its statistics are ready. It warms each query twice, then times it 20 times, using
in-process HTTP to the last byte. The answer hashes were identical across every run and arm.

| Query | pre | #238 | branch | branch / pre |
| --- | ---: | ---: | ---: | ---: |
| `all_C3` | 13.27 ms | 14.84 ms | 7.81 ms | **0.59** |
| `distinct_C3` | 41.07 ms | 14.81 ms | 7.87 ms | **0.19** |

The other 18 agg-WatDiv stars and both non-star controls take 0.16–1.1 ms on every arm. At that size the arms differ
by up to about 0.06 ms whatever path a query takes, so those ratios can't be attributed. The worst branch/pre ratios
are `distinct_S1` (1.23, +0.06 ms) and `all_L4` (1.15, +0.06 ms). Several are clearly lower, such as `distinct_S5`
(0.39) and `all_S3` (0.56). All rows are in `runs.txt`.
