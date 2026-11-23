# Characteristic pairs and sort-then-probe: laptop A/B on BSBM-100M BI (task #105)

The test store is a copy-on-write clone of the BSBM-100M four-order store. Its statistics are rebuilt at open, so the
pairs are present. The run used one release binary and four arms through the server's `--ablate=` list:
- `all`: pairs, sorted probe, and a probe-cost planner model (a later version dropped this model);
- `nosort`;
- `nocost`: pairs plus the sorted probe, which is what this branch ships;
- `base`: all three off.

The blocks ran in the order all, nosort, nocost, base, base, nocost, nosort, all. Each block ran a 30-second statistics
warm-up, then each of the 60 frozen BI requests once untimed and twice timed. Response bytes were SHA-256-identical
across every run and arm. The numbers are laptop mechanism evidence only.

| Arm | Geomean against base (60 BI) | q1 m000 | q1 m001 | q3 m001/m002/m003 |
| --- | ---: | ---: | ---: | --- |
| pairs + sorted probe (`nocost`) | **0.975** | 33.3 ms | 57.9 ms | 790 / 1,291 / 1,026 ms |
| + probe-cost model (`all`) | 1.030 | 33.9 | 58.1 | 1,502 / 2,197 / 2,170 ms |
| pairs + probe cost, no sorted probe | 1.029 | 35.2 | 62.9 | 1,502 / 2,202 / 2,174 ms |
| base | 1 | 140.3 | 54.2 | 783 / 1,260 / 1,020 ms |

- **q1 m000 is 0.24× of base.** Its producers are in AT and its reviewers in US. With pairs, the planner starts from
  `?producer country AT`. Without them, it can't tell producers' `country` from reviewers' and starts from the US
  reviewers, carrying 1.14M rows through four joins.
- **The probe-cost model flipped the q3 plans and slowed them about 2×,** so it is not shipped.

`bi-ab-v2.json` has every request's timings and response hashes per arm.
