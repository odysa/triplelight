# Corrected QLever baseline (task #105)

The QLever arm of the #105 baseline profile, rerun on the same six suites, the same stores and frozen references, with QLever's
result cache emptied before every request (warm-up included; the driver of `bench/eval`, PR #199). Same settings as the baseline:
3 timed runs after one warm-up, 120 s timeout, `--mem 12 --cpus 8`, Docker on the laptop (macOS arm64), harness at main 69fe7933.
One session, one run per request. Raw timings and answer classes per suite are in `<suite>/qlever.warm.json`; the harness head is
in `results.txt`.

Answer classes are unchanged from the baseline (the cache changes timings, not answers): BSBM BI 25 pass / 12 value-compatible /
23 mismatch, BSBM Explore 7 / 13, LUBM 14 pass, LUBM aggregates 30 pass, WatDiv aggregates 42 value-compatible, WatDiv 20 pass.

Geometric mean over the requests that passed or were value-compatible in both runs (median of 3 timed runs per request):

| Suite | requests | baseline (cache on) | corrected (cache cleared) | corrected / baseline |
| --- | ---: | ---: | ---: | ---: |
| BSBM BI (bi100) | 37 | 3.7 ms | 83.5 ms | 22.8× |
| BSBM Explore (bsbm100) | 20 | 14.5 ms | 48.2 ms | 3.3× |
| LUBM-1000 | 14 | 20.3 ms | 53.9 ms | 2.7× |
| LUBM aggregates | 30 | 3.1 ms | 28.2 ms | 9.2× |
| WatDiv aggregates | 42 | 9.5 ms | 16.2 ms | 1.7× |
| WatDiv | 20 | 12.0 ms | 18.5 ms | 1.5× |

Largest single-request ratios: BI 546×, LUBM aggregates 833×, LUBM 22.7×, Explore 5.9×, WatDiv aggregates 7.9×, WatDiv 2.3×.

## Limits
Laptop Docker, one session; the baseline's TripleLight and Virtuoso columns were not rerun and come from the original profile. The
23 BI mismatches are excluded from the BI geometric mean (as in the baseline's classification), so the 37-request BI figure is
not the whole suite.
