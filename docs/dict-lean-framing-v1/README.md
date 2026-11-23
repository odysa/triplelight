# `RDFDID03`: lean dictionary entry framing (#105, storage lane)

**The problem:** the dictionary census (`docs/dictionary-census-v1/`) found a fixed 16 bytes of framing per entry
(an 8-byte ID and two 4-byte lengths), 376 MB on BSBM-100M. 99.6% of entries have the previous entry's ID + 1.

**The change:** in `RDFDID03`, each entry is `varint(ID − previous ID)`, where the block's first entry stores
`varint(ID)`, then `varint(prefix)`, `varint(suffix length)` and the suffix. That's usually 3 bytes of framing. The
text, the front coding, the blocks, their checksums and the directory are unchanged.
- `RDFDID02` stays readable.
- Older binaries refuse `RDFDID03` by its magic.
- IDs must ascend within a block, and a zero delta or a noncanonical varint is refused.

**Sizes** (`sizes.txt`; four-order stores built from the same inputs, index files byte-identical between the arms):

| | `RDFDID02` | `RDFDID03` | ratio |
| --- | ---: | ---: | ---: |
| LUBM-1000 dictionary | 672.1 MB | 245.4 MB | **0.37** |
| LUBM-1000 store total | 2.62 GB | 2.20 GB | 0.84 |
| BSBM-100M dictionary | 4,620.2 MB (`docs/codec-rotate-v1`) | 4,319.1 MB | 0.93 |
| BSBM-100M store total | 6.16 GB | 5.86 GB | 0.95 |

What the numbers show:
- **LUBM is IRI-heavy,** and front coding already removes almost all of its IRI text, so framing was most of the
  file.
- **BSBM saves 301 MB,** which matches the census estimate of about 290 MB. The rest of BSBM's dictionary is literal
  text, the subject of the next measurement.

**Output latency** (`replay.sh`: q1–q14 in the #105 baseline order, one server lifetime per session, curl to
`/dev/null` timed to the last byte as in B8, `--cache-mb 6144`; ABBA main `67aab080` / lean / main / lean). Response
bytes are identical across all four sessions for every query.

| Query | timed median, main | lean | ratio | first request (warm-up), main | lean |
| --- | ---: | ---: | ---: | ---: | ---: |
| q13 (2,604 output terms) | 7.5 ms | 7.0 ms | 0.94 | 134.4 ms | 65.2 ms |
| q2 | 25.4 ms | 25.2 ms | 0.99 | 658.4 ms | 508.6 ms |
| q8 | 10.6 ms | 10.2 ms | 0.96 | 597.7 ms | 495.9 ms |
| q9 | 670.6 ms | 571.7 ms | 0.85 (noise 1.07) | 1,074 ms | 1,047 ms |
| q6 / q14 (7.9M rows) | 1,176 / 1,271 ms | 1,184 / 1,239 ms | 1.01 / 0.97 | 2,106 / 1,395 ms | 1,993 / 1,314 ms |
| other queries | | | 0.93–1.01 | | |

How to read the table:
- **Warm requests are unchanged:** within noise, or slightly faster.
- **The first request to touch a block is faster,** because there are fewer bytes to read and hash. q13's first run
  is 0.49×, and q2's and q8's 0.77× and 0.83×.
