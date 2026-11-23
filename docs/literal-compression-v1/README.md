# Literal-text compression, measured offline (#105, storage lane; no format change)

The dictionary census (`docs/dictionary-census-v1/`) found literal text to be 90% of BSBM-100M's dictionary. This
measures how far real literal blocks compress before any format change.

**Sample:**
- Every 20th literal block of the #105 baseline's BSBM-100M unified dictionary, exactly as stored, dumped by the
  ignored test `lazy_dictionary::dump::dictionary_dump_literal_blocks` (branch `storage/dict-census`). That's 1,711
  of 34,232 literal blocks, 215.9 MB, about 126 KB per block.
- They are split by alternating blocks into a training half and a test half. Every ratio below is on the **test**
  half: 855 blocks, 108,217,264 bytes. Each block is compressed on its own, as a per-block format would store it.
- Tools: zstd and lz4 command-line tools (Homebrew, laptop). Trained dictionaries are `zstd --train` on the training
  half only.

| Encoding, per block | Compressed / stored | Decompression (one core, laptop) |
| --- | ---: | ---: |
| lz4 (default) | 0.785 | |
| zstd -1 | 0.484 | |
| zstd -3 | **0.473** | 760 MB/s |
| zstd -9 | 0.437 | |
| zstd -19 | 0.419 | |
| zstd -3, 110 KB trained dictionary | 0.436 | |
| zstd -9, 110 KB trained dictionary | 0.397 | |
| zstd -3, 1 MB trained dictionary | **0.412** | 595 MB/s |
| zstd -9, 1 MB trained dictionary | **0.333** | |

**What it means for BSBM-100M,** scaling the test half's ratios to the 4.3 GB of literal blocks:
- **zstd -3 per block:** about 2.3 GB saved, so the dictionary goes from 4.3 GB to about 2.0 GB.
- **zstd -9 with a 1 MB trained dictionary:** about 2.9 GB saved, to about 1.4 GB.
- **The store:** 5.86 GB becomes about 3.5 GB or 3.0 GB respectively.
- **lz4 is not worth it here:** 0.785. Generated BSBM text is word-level redundant, which needs entropy coding.

**The read cost:**
- **Per miss:** one block decompression, about 126 KB at 600–760 MB/s, so about 0.17–0.21 ms. That's before the
  current parse, which stays.
- **Where it applies:** only literal blocks would be compressed. IRI blocks, which are almost all front-coded and
  what LUBM's output decodes, would be unchanged.
- **What to measure in an A/B:** the queries that output many literals on first touch, BSBM explore and BI, and LUBM
  queries returning names and emails.

**Recommendation:**
1. **Per-block zstd -3 for literal blocks,** with a flag per block. That's the larger half of the win at the lowest
   risk, and it needs no trained dictionary.
2. **A trained dictionary per datatype or language** is the follow-up: a further 13–30% (0.412–0.333 against 0.473).
   It costs a dictionary blob stored once per base and a training step at build time.
3. **The format:** both belong under a new dictionary version (`RDFDID04`), with `RDFDID03` still read. Each also needs
   a zstd dependency, which means a C library in the build. It's already common in Rust, but the wasm binding needs a
   check.
