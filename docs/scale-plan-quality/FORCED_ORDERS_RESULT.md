# Negative result: large q-error does not leave an order improvement here

For both Q9 and Q2 on one in-memory LUBM-1000 build, the current optimizer selected **Y → Z → X**, and that was the fastest of all six leapfrog variable orders. The selected/best median ratio is **1.00× for both**, below the pre-stated 1.5× engineering-payoff threshold. Leave these queries' estimator and order heuristic unchanged.

| Forced order | Q9 median [min, max], ms | Q2 median [min, max], ms |
| --- | ---: | ---: |
| X → Y → Z | 1837.014 [1777.248, 1898.429] | 2403.748 [2258.926, 4138.951] |
| X → Z → Y | 3453.742 [3304.478, 3681.241] | 2177.269 [2082.117, 2595.543] |
| Y → X → Z | 443.346 [400.222, 473.252] | 2285.391 [2169.036, 3402.958] |
| Y → Z → X | 322.122 [313.958, 375.968] | 18.066 [16.423, 28.292] |
| Z → X → Y | 4135.305 [3967.116, 5991.887] | 706.244 [639.491, 1254.780] |
| Z → Y → X | 946.725 [898.610, 1235.498] | 51.467 [43.389, 64.332] |

Each order has six measured runs, following one warm-up. The measured passes rotate and reverse order to reduce a fixed-position advantage. In this session the selected order's slowest sample was still faster than the next-best order's fastest sample for both queries. This is a decision-sensitivity result on these queries, not a confidence interval or a universal optimum claim.

All **84 samples** (72 measured, 12 warm-up) completed. Every sample matched the ordinary statistics-enabled execution's **complete sorted ID multiset**, preserving multiplicity: Q9 44,190 rows and Q2 2,528 rows, also the counts in the earlier Jena reference. Raw timings and per-query hashes are in forced-lubm1000/raw/samples.tsv. No incomplete run or timeout was treated as a successful sample.

## What was held fixed

Measured source **aafa7b2**, preserved as tag **task17-orders-measured**, includes the production exec/bgp.rs file directly in the example. Orders enter the existing given-order path; this is not a copied implementation or a new production routing feature. All orders share one memory snapshot and Stats instance. The chosen order comes from the same gate and choose_variable_order code as the ordinary runtime. Sorting, hashing, remapping the ordinary result by variable name, validation and output are outside the kernel timer. The ordinary query API's 295 ms / 52 ms samples are descriptive setup observations, not the denominator for the comparison.

The immutable input SHA-256 is **43a7c3bd5ba7ba6e7e14b0a289b69303e82035372d1750f88bac059ab70439c0**. The 133,613,894-triple input, helper/kernel/query sources and binary are recorded in receipt.json; input and binary hashes were unchanged after the run. The process exited 0 within its 900-second limit.

Memory loading took 214.14 s with 10 parsing workers; Stats construction took 24.24 s. The complete process took 413.84 s. macOS time -l recorded maximum RSS 32,908,296,192 bytes (30.65 GiB) and peak memory footprint 50,173,828,384 bytes (46.73 GiB). These are different OS metrics and include graph construction, not just query execution. Raw resource output is preserved.

## Interpretation and limits

The earlier disk-snapshot readout's Q9 q-error of roughly 19,074 and Q2 q-error of 21.8 did not justify changing an execution decision. This memory experiment rebuilt the dictionary and synopsis; ID-dependent sketches can differ between builds. It tests the selected order for its own build, and does not prove every future reload chooses the same order.

The order heuristic scores projected variable prefixes, so a poor complete-BGP cardinality alone need not make the order poor. The controlled six-order comparison establishes that no faster tested order was left on the table in this session. It does not establish that q-error is harmless for other queries, backend choices or future data.

This was one session on a shared host. The RDF benchmark lock serialized the team's builds/timings, but does not establish machine-wide isolation. Only the kernel is timed; HTTP transfer, RDF-term decoding and disk/dictionary-cache behavior are outside this payoff bound. The observed memory result should not be used to explain the disk cache regression.

Next, as requested by Kaiming, screen the slow WatDiv and BSBM-100M BGPs for actual leapfrog eligibility and variable count before applying the same payoff test. Do not force a different join algorithm and label its result an order improvement for the production query.
