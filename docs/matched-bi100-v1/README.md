# Matched-VM BSBM BI100M: two passes on qualified ef7a778

rdfdb has the lowest observed weighted total in both passes of this registered workload, under the unchanged **value-compatible** gate. All **360 requests** pass that gate. QLever takes **2.06× / 3.64× as long** as rdfdb; the plain-triples DuckDB translation takes **48.85× / 65.90× as long**. The substantial within-session variation remains visible; neither pass is selected as the preferred result.

| Engine | Forward, seconds | Reverse, seconds |
|---|---:|---:|
| rdfdb disk, ef7a778 | **11.744** | **6.730** |
| QLever, 18c097c | 24.176 | 24.506 |
| DuckDB 2.0.0, plain typed triples | 573.758 | 443.532 |

Each total sums the same 45 scored HTTP requests after 15 warmups. There are two ordered observations, not a confidence interval, official BSBM 50/500-mix throughput measurement or general SOTA proof. The DuckDB result applies to the published lossless plain-triples schema and registered SQL translation, not every possible DuckDB schema or tuning. All raw attempts are retained; no timeout, OOM, missing request or failed sample was discarded.

## What still favors QLever

q3 remains faster in QLever in both passes. q1 changes winner between passes, so there is no stable q1 lead. These are sums over each template's registered scored instances, not repeated identical-query medians:

| Template | rdfdb forward / reverse, ms | QLever forward / reverse, ms |
|---|---:|---:|
| q1 | 1,492 / 848 | 1,017 / 967 |
| q3 | 8,988 / 5,106 | **2,413 / 2,536** |

q3 accounts for about 76% of rdfdb's scored total in each pass and is the clearest next latency target. The timing does not establish its cause; changes to join order, allocation or caching need their own controlled investigation. Detailed sums and instance counts for every template are in `round1/template-summary.csv`.

## Qualification is conditional on the registered numeric rules

| Engine | Primary, each pass | COUNT-compatible, each pass | Value-compatible, each pass | Exact RDF-term bags, each pass |
|---|---:|---:|---:|---:|
| rdfdb | 60/60 | 60/60 | 60/60 | 58/60 |
| DuckDB | 60/60 | 60/60 | 60/60 | 42/60 |
| QLever | 25/60 | 37/60 | 60/60 | 25/60 |

A primary pass already includes frozen numeric tolerances. The value gate additionally permits specifically registered numeric datatype and rounding differences; it does not establish strict RDF-term or global SPARQL equivalence. Other identities, binding state, multiplicity, cutoff membership and ordering remain checked. QLever has no eligible whole-cohort primary or COUNT-only timing total; its table entry is explicitly value-compatible.

The numeric input freeze is `63e127b60ac8de510a172748108639f0834e17bc3772e1d16ce226404aa58cca`; COUNT policy `5049fb75210fc6fc78ebf1fb2553b88baef197ae73495b70f110a203a70a4663`; value policy `7b3f640bbab516db80ceba87549b9670802a93c5ff77181bdda66c0855fc3dfb`. Rules were registered before the 100M candidate outputs. The timing protocol was fixed after separate output qualification and before these timed responses. This previously seen, version-pinned workload is not a blind holdout, and no bound or reference was widened in response to timing results.

References remain 59 original full-query Jena responses and one explicitly group-key-partitioned q1 response, independently validated as documented in `../jena-bi100-reference-v1/`. The actual input has 98,861,444 unique triples; nominal 100M means the BSBM generator scale, not an exact triple count. All stores bind to NT SHA256 `6e4ef939f05b15677802d17257096de9da32a452fe33dd40c07d0b0537ee430d`.

## Runtime and measurement scope

All engines ran serially in the same ARM64 Docker VM on the shared Mac host, each with a four-CPU quota and 12 GiB container memory cap, with memory+swap also capped at 12 GiB. These are CPU quotas, not dedicated physical cores. Engine settings were rdfdb 10240 MB shared cache, DuckDB 8 GB working / 8 GB spill allowance and QLever 11 GB query memory. The per-request deadline was 600 seconds. OS page caches were not cleared, and the host was not a dedicated benchmark machine.

Order was rdfdb → DuckDB → QLever, then QLever → DuckDB → rdfdb. Stores were prebuilt and mounted read-only. rdfdb statistics readiness was required before requests. QLever used its default non-hash GROUP BY profile with 1 MB result-cache caps and explicit cache clear before every request; all 120 acknowledgements return zero for all five cache counters. The DuckDB adapter redoes translation, planning, execution, fetch and serialization inside each HTTP request. The timer covers the host request through full response-body consumption; cache control, JSON parsing and qualification are outside it. Build, import, readiness and warmup time are excluded from the scored sums. No load/build comparison is inferred.

The source is `ef7a77814d23879a7bdf174ffe567b81a402ba9f`. The Linux executable SHA256 is `14c83f93abb778a3c99fb6f287b3995a1f98dd832cdfa12432736d77ba45c34d`. It was freshly built inside the pinned Rust image from copied, hash-verified sources; the Docker die receipt records exit 0. Reused target artifacts were copied into a new directory, leaving the old binary and target unchanged. Both RDF passes independently qualify their actual outputs; later engine fixes are separate experiments, not relabelled as this build.

The runtime images, source and adapter hashes, compiler versions, Linux DuckDB module hashes, input file identities, commands and protocol SHA256 `de7f1a84d4343494d4e149e3245caa423f024a9530d63532539383fabfeb5ff9` are retained. Databases, index data and executable blobs are kept locally rather than committed.

## Container memory scope

| Engine | Forward peak, GiB | Reverse peak, GiB |
|---|---:|---:|
| rdfdb | 3.688 | 4.077 |
| QLever | 0.511 | 0.393 |
| DuckDB | 11.357 | 11.996 |

These are Linux **cgroup memory.peak** observations including startup/readiness/warmup and cgroup-charged caches and kernel memory, **not process RSS**. QLever has the lower measured container peak. DuckDB approaches the cap; the effect on latency is not established. The figures must not be pooled with earlier native or VM process-RSS measurements or treated as hard application-memory guarantees.

All six clients exit 0; every retained container state has OOMKilled false, and all owned containers and client/runner processes exited before the owner lock was released. Input, source, adapter and module identities stayed unchanged. Sampled guards observed a maximum owned-root allocation of about 16.01 GiB and minimum shared free space of about 228.00 GiB, within the approved limits. Two-second sampling and named-file accounting do not prove an instantaneous bound or zero spill.

## Evidence and next work

`round1/` retains all 360 raw response bodies, 120 cache acknowledgements, transport and separate gate records, commands, readiness/resource/cleanup receipts, guard samples, before/after full input hashes and the replayed `postchecks.json`. `postcheck.py` replays the policies and checks totals, source identities and acknowledgements. `file-sha256.json` binds the complete archived evidence.

Next: investigate q3's repeated deficit and the measured container-memory gap, with independent qualification and controlled A/B evidence. The curated 72 aggregate suite and tuned Ladybug comparison remain pending. These results do not complete the broader analytical SOTA objective.
