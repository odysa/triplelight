# Dictionary misses outside the shard lock (#105, storage lane)

**The change:** a dictionary block-cache miss used to read the block, check its SHA-256 and decode its terms while
holding the cache shard's mutex, which is one of 16. Now the shard lock is held only for the lookup and the insert.
Two threads missing the same block may both decode it, and the first insert wins. Blocks are immutable, so either
copy is correct.

**Setup:**
- Laptop, one session.
- One LUBM-1000 four-order store (`RDFIDX04`, `RDFDID03`) built by main.
- `replay.sh` (q1–q14 in the #105 baseline order, curl to `/dev/null`, `--cache-mb 6144`), alternating
  main / branch three times. Main is `141b0735`.
- The table gives the medians of three sessions per arm, with their ranges. Response bytes are identical in all six
  sessions.

| Query | first request, main | branch | ratio | timed median ratio |
| --- | ---: | ---: | ---: | ---: |
| q9 | 952.9 ms (910–964) | 827.1 ms (804–836) | **0.87** | **0.87** |
| q6 (7.9M rows) | 1,895.8 ms (1,881–1,944) | 1,847.0 ms (1,824–4,022) | 0.97 | 1.03 |
| q14 (7.9M rows) | 1,311.7 ms (1,311–1,357) | 1,371.8 ms (1,305–1,468) | 1.05 | 0.99 |
| q2, q8, q13 | | | 0.97–1.00 | 0.85–1.08 |
| other queries (sub-ms) | | | 0.90–1.20 | 0.99–1.09 |

**Reading it:**
- **q9 gains about 13%,** in both its first and its warm requests, with non-overlapping ranges across sessions.
- **q6 and q14 (large outputs) are within noise.** So the shard lock was not what bounds their parallel output
  decoding on this machine; one branch session had an outlier at 4,022 ms.
- **The rest are within noise.**

The change is small and keeps behavior identical, so it is worth keeping for the q9 gain and for removing a
serialization point. It is not a large-output fix.
