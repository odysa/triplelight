# Dictionary cache: admit a block on its second miss (#105, storage lane)

**The problem:** in the #105 baseline, LUBM-1000 q13 (2,604 output rows, one term per dictionary block) took 41–55 ms,
almost all of it output. In a fresh server, the same query's repeats take 2.4–4 ms (WindowsXP's trace). The cause is
the dictionary cache's admission rule:
- A miss into a full shard is admitted only with probability 1/4 (`ADMIT_ONE_IN`, the scan resistance added in B2).
- By the time q13 runs, q1–q12 have filled the shards. Each q13 run then admits only about a quarter of the blocks it
  still misses.

**The change:** a rejected miss's key goes into a bounded ghost set: FIFO, at most 2 keys per resident block (at
least 256), keys only. A second miss on a ghost key is admitted for certain. A first miss keeps the 1/4 rule, so a
one-shot pass still can't flush the hot set. This is TinyLFU's doorkeeper idea, without a frequency sketch.

**Setup:**
- Laptop, one session.
- `replay.sh`: q1–q14 in the baseline's order on one server lifetime, each with 1 warm-up and 3 timed runs.
  `--cache-mb 6144`, as the baseline had at 12 GB, on a four-order LUBM-1000 store.
- ABBA: main `293f2d4d`, then branch, then main, then branch. Times are curl totals (`times-*.tsv`). The traces
  (`trace-*.txt`) have per-request dictionary hits and misses. Response bytes are identical across all four
  sessions for every query.

**q13, dictionary misses per request** (warm-up, then timed 1–3; `trace-main.txt` / `trace-ghost.txt`, lines 50–53):

| | warm-up | timed 1 | timed 2 | timed 3 |
| --- | ---: | ---: | ---: | ---: |
| main | 658 | 506 | 375 | 291 |
| ghost | 657 | 464 | 5 | 2 |

**Timed medians, the mean of both sessions per arm:**

| Query | main | ghost | ratio | main against main |
| --- | ---: | ---: | ---: | ---: |
| q13 | 43.2 ms | 6.9 ms | **0.16** | 1.00 |
| q12 | 2.0 ms | 1.2 ms | 0.60 | 1.40 (noisy) |
| q6 (7.9M rows) | 1,203 ms | 1,213 ms | 1.01 | 0.97 |
| q14 (7.9M rows) | 1,291 ms | 1,366 ms | **1.06** | 0.97 |
| q9 | 705 ms | 709 ms | 1.01 | 1.02 |
| other queries | | | 0.96–1.03 | |

**The cost:** q14 is about 6% slower in this sequence. Its warm-up and first timed run miss more blocks (11,703 and
10,306 against main's 7,018 and 5,752) because q13's blocks now hold their places. By its third timed run, q14
misses fewer blocks than on main (2,985 against 3,798). Over q14's three timed runs, the misses are 18,069 against
14,275. Large outputs such as q6 and q14 are bound by the driver in the harness (#205), and the gain on a repeated
small-result query is 6×. A frequency-based admission (a TinyLFU sketch comparing the candidate with the victim) could
recover q14's cost and is the follow-up if it matters.

**Accepted as a documented trade (Kaiming, #105).** The q14 cost comes from sharing the cache between two queries, not
from extra work on q14's path, and the net is strongly positive. The TinyLFU sketch is the named follow-up, to be done
if a rental suite shows the q14 pattern mattering. The replay times are curl to `/dev/null`, timed to the last byte
with no hashing, the same as #205's drained timing.
