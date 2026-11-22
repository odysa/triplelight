# Where a four-order LUBM-1000 disk build spends its time (task #89, part 1)

One instrumented external build of `lubm1000.nt` (138,318,414 lines, 133.6M triples) with the default four orders.
Registered before running (task #89); scored below. Single session, one host; descriptive.

## Setup

- **Code:** main at `683ac664` plus `phase-logging.patch`, which only adds `eprintln!` calls that print a timestamp and
  the build directory's size at each phase boundary and for each order (rows read, sorted, counted, written).
- **Command:** `triplelight build-disk lubm1000.nt DIR --build-external`, under `/usr/bin/time -l`, with a sampler that
  lists the output directory every 2 s (`raw/files.log`).
- **Result:** 654.8 s wall, 1,324.9 s user and 552.3 s system CPU (an average of 2.9 of the host's 10 cores busy),
  maximum RSS 2.31 GB, store 3.0 GB (`RDFSNAP6`). Peak size of the build directory, from the sampler: 46.7 GB.

## Phases

| Phase | Seconds | Share of wall |
|---|---:|---:|
| Parse the N-Triples and sort terms in shards | 253.6 | 38.7% |
| Merge terms and write the dictionary pass | 94.0 | 14.4% |
| Sort the term-ID mapping | 77.5 | 11.8% |
| Write the raw ID triples | 12.9 | 2.0% |
| Convert the dictionary to the unified file | 43.7 | 6.7% |
| Orders SPO, PSO and POS, three at once | 115.0 | 17.6% |
| Order OSP, alone | 57.8 | 8.8% |
| Remove scratch and finish | 0.3 | 0.0% |

Inside the order phase (seconds from the start of each order's work):

| Order | Read rows and push keys | Sort and merge | Count unique | Write index |
|---|---:|---:|---:|---:|
| SPO (with PSO, POS) | 40.2 | 41.2 | 7.4 | 24.0 |
| PSO (with SPO, POS) | 48.7 | 41.1 | 7.0 | 13.6 |
| POS (with SPO, PSO) | 54.0 | 42.2 | 6.7 | 11.8 |
| OSP (alone) | 25.0 | 21.5 | 4.0 | 7.2 |

(The sort and merge column runs from the end of the read to the sorted run being complete; the three orders overlap.)

## Cross-check against #88

Task #88's sampler (a separate set of runs of the same command and data, on the same host) gives 659.6 s and 616.0 s wall
for two four-order LUBM-1000 builds, and its file timeline has the dictionary file appear and then the first three
orders within about 18 s of each other, and the fourth about 60 s later. Mine: the unified dictionary at 451 s, SPO, PSO
and POS at 570, 578 and 584 s, OSP at 647 s, so the fourth order starts 63 s after the third. The two timelines agree on
the shape and, once shifted to a common origin, to within about 15 s.

## Predictions

- **1a (the order builds are 30 to 45% of the wall time):** missed. They are 172.8 s, 26.4%.
- **1b (parse and term sort together are the largest piece, at least 30%):** held. The shard parse and sort alone is 38.7%.
- **1c (the fourth order runs alone as a single-thread tail, 25 to 40% of the order phase):** held. The lone OSP build is
  57.8 s, 33.4% of the order phase. It is also about twice as fast as an order built beside two others (57.8 s against
  115 s for three), so three orders at once finish in 115 s against about 173 s in series (3 × 57.8), a 1.5× gain for the batch, while each order runs about 2× slower than alone.

## What it says

- Everything after the dictionary is 26% of the wall time. Even if the whole order phase (reading, sorting, counting and
  writing) became three times faster, the build would be about 17% shorter (115 s). The larger pieces are the parse (39%) and the dictionary and mapping stages together (33%:
  the term merge and dictionary pass, the mapping sort, the raw rows and the conversion).
- The 46.7 GB peak of the build directory is in the parse phase (at 244 s, from the sampler), when the shard runs for
  the input are on disk; the order phase peaks at about 20 GB. Any reduction of scratch space has to target the parse.
- The machine is far from busy: 2.9 cores on average, and 29% of the CPU time is system time, so the build is also
  waiting on and issuing a great deal of I/O.
- Running the four orders as one batch of four, or two batches of two, would remove the lone 57.8 s tail only if four
  concurrent sorts don't slow each other down more than three do. That was not measured here.

## Limits

- One build and one host; the same command in #88 varied by about ±5% between repeats (616 s and 660 s).
- The phase boundaries in the order phase are for the first run only, and the three concurrent orders share the same disk
  and CPUs, so their individual times are not independent.
- No change to the builder was tried; part 2 (sort microbenchmark) is reported separately.
