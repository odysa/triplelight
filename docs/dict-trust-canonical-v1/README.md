# Dictionary read path: no canonical re-serialization on block misses (#105, storage lane)

**The problem:** `term_block_codec::decode` parsed every term of a block and then re-serialized it, refusing the
block if the text differed from its canonical form. It did that for all 256 terms, on every dictionary block-cache
miss. That is validation only: the build writes canonical text, and the block's SHA-256 and the parse already reject
corrupted bytes.

**The change:** the read path (`LazyIndex::block`) decodes with `canonical = false`, so it still parses but no longer
re-serializes. `check_text_order`, which `verify` runs, now reads every block directly and decodes it with the full
canonical check before its order checks. So `verify` refuses noncanonical text exactly as before. A unit test covers
noncanonical but parseable text (refused by the canonical decode, accepted by the read path) and unparseable text
(refused by both).

**Setup:**
- Laptop, one session.
- The same four-order LUBM-1000 store (`RDFIDX04` and `RDFDID03`, built by main) for both arms.
- `replay.sh`: q1–q14 in the #105 baseline order, curl to `/dev/null` as in B8, `--cache-mb 6144`.
- ABBA: main `f9a4194e`, then the branch, then main, then the branch.
- Response bytes are identical in every session.

| Query | warm-up (first touch), main | branch | ratio | timed median ratio |
| --- | ---: | ---: | ---: | ---: |
| q8 | 491.5 ms | 181.0 ms | **0.37** | 0.99 |
| q2 | 510.3 ms | 379.3 ms | 0.74 | 1.00 |
| q13 | 64.8 ms | 48.8 ms | 0.75 | 0.95 |
| q9 | 1,047.7 ms | 898.0 ms | 0.86 | 0.91 |
| q6 (7.9M rows) | 1,981.3 ms | 1,778.8 ms | 0.90 | 1.01 |
| q14 (7.9M rows) | 1,314.9 ms | 1,287.0 ms | 0.98 | 1.00 |
| other queries | | | 0.66–1.06 (sub-ms) | 0.95–1.12 (sub-ms) |

Main against main is within 2% on the warm-ups above and within 10% on sub-millisecond requests.

**How to read it:**
- **Warm requests barely move:** their dictionary blocks are already cached and decoded.
- **The first request to touch a block gains:** q8 by 2.7×, because each of its misses no longer re-serializes 256
  terms.
- **`verify`'s cost is unchanged:** it already decoded every block, and it now checks canonical text in a separate
  direct pass.
