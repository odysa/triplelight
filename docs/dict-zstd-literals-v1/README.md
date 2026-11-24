# `RDFDID04`: zstd-compressed literal dictionary blocks (#105, storage lane)

**The change:** each dictionary block starts with a codec byte. A literal block is stored as one zstd frame (level 3)
when that is smaller, and every other block stays raw (`term_block_codec::encode_tagged`).
- `RDFDID03` and `RDFDID02` stay readable.
- Older binaries refuse `RDFDID04` by its magic.
- A zstd frame may not decompress past the largest lean block.
- The zstd library is linked only on Unix, where disk stores exist. The wasm build doesn't link it: `cargo tree -i
  zstd-sys --target wasm32-unknown-unknown` is empty, and the wasm binding builds as before.
- The licence is in `THIRD_PARTY_NOTICES.md`.

This follows the offline measurement in `docs/literal-compression-v1/` and Kaiming's conditions (#105).

**Sizes** (`sizes.txt`; four-order stores built from the same inputs by main `6d3d4179` and the branch; index files
byte-identical):

| | main (`RDFDID03`) | `RDFDID04` | ratio |
| --- | ---: | ---: | ---: |
| LUBM-1000 dictionary | 245.4 MB | 154.9 MB | 0.63 |
| LUBM-1000 store | 2.20 GB | 2.11 GB | 0.96 |
| BSBM-100M dictionary | 4,319.1 MB | 2,104.0 MB | **0.49** |
| BSBM-100M store | 5.86 GB | 3.65 GB | **0.62** |

**Latency:**
- **Setup:** ABBA (main / branch / main / branch), one server lifetime per session, `--cache-mb 6144`, curl to
  `/dev/null` timed to the last byte, as in B8. Each query had 1 warm-up and 3 timed runs.
- **Queries:** LUBM-1000 q1–q14 (`replay.sh`), and the 60 frozen BSBM BI requests (`replay-dir.sh`).
- **Answers:** response bytes are identical across all four sessions for every request.

| Suite | first request (warm-up), geomean | timed median, geomean | main against main |
| --- | ---: | ---: | ---: |
| BSBM BI (60) | **0.896** | 1.001 | 1.009 |
| LUBM-1000 (14) | 0.967 | 1.001 | 1.005 |

Largest first-request changes on BI:
- m000_p13_q7: 63.5 → 18.7 ms (0.29×)
- m002_p12_q8: 78.9 → 50.5 ms (0.64×)
- m000_p04_q4: 636 → 416 ms (0.65×)
- m000_p08_q5: 200 → 133 ms (0.67×)
- m000_p00_q1: 260 → 190 ms (0.73×)

No request is slower beyond noise on first touch. On LUBM, q8 is 0.82× and the rest are within noise.

**How to read it:**
- **Decompressing costs less than it saves.** A literal-block miss now reads and SHA-256-hashes about half the bytes
  and then decompresses them. That's a net gain on first touch.
- **Warm requests don't change:** their blocks are already decoded in the cache.
- **The store-size table for the paper moves again.** BSBM-100M four-order is now 3.65 GB, so record it at Kaiming's
  format freeze.

**Next, measured separately:** zstd with a dictionary trained per datatype or language, worth about 13–30% more on
literal blocks (`docs/literal-compression-v1/`).
