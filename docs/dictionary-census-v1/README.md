# Dictionary census, BSBM-100M (#105, storage lane; measurement only)

The dictionary is 4.62 GB of BSBM-100M's 6.16 GB four-order store. The census is the ignored test
`lazy_dictionary::census::dictionary_census` (branch `storage/dict-census`), run read-only on the #105 baseline's
BSBM-100M store. It re-derives what the unified file's bytes are spent on, from every block's decoded entries.

The format today (`term_block_codec`): blocks of up to 256 entries in ID order. Each entry is an 8-byte ID, a 4-byte
shared-prefix length, a 4-byte suffix length, and the suffix bytes, front-coded against the previous entry's text.
There is no compression beyond front coding.

| | IRIs | Literals |
| --- | ---: | ---: |
| terms | 14,739,490 | 8,763,181 |
| text if stored whole | 1,334.96 MB | 4,278.43 MB |
| removed by front coding | 1,299.17 MB (97%) | 97.68 MB (2%) |
| suffix bytes stored | 35.79 MB | 4,180.75 MB |
| per-entry framing (16 B) | 235.83 MB | 140.21 MB |
| entries whose ID is the previous + 1 | 99.6% | 99.6% |
| leaner framing estimate (varint lengths, consecutive IDs implied) | 65.73 MB | 4,202.21 MB, with 21.5 MB of framing |

The file is 4,620.21 MB in 91,809 blocks.

**Where the bytes are:**
- **Literal text is 90% of the dictionary: 4.18 GB of suffix bytes.** Front coding does almost nothing for it, because
  consecutive literals in text order share little beyond their first characters.
  - By type: `@en` is 1.75 GB (1.42M terms), `xsd:string` 0.69 GB (4.91M), and seven other languages about 1.76 GB
    together (zh, ja and de at about 0.35 GB each).
  - Lengths: 4.0M literals are 256–1,023 characters, and 1.96M are 1–4 KB. These are BSBM's generated product and
    review texts.
- **IRIs are almost entirely front-coded away:** 36 MB of suffix text. Their cost is the framing, 236 MB, or 16 bytes
  per term.

**What this implies, in order of size:**
1. **Compress literal suffixes within a block,** for example zstd or LZ4 per block, or per 64 KB run of literal
   blocks. BSBM's texts are drawn from a fixed vocabulary, so they should compress several times over; that has to be
   measured. This is the only lever that moves the dictionary by gigabytes. Decoding a block then costs a
   decompression, so it needs an A/B on the output-heavy queries (LUBM q13, BI) as well as the size.
2. **Leaner framing:** varint lengths, and the ID implied when it is the previous + 1 (99.6% of entries). That saves
   about 290 MB: 236 → 66 MB for IRIs and 140 → 22 MB for literals. It is cheap to decode, and is a format change
   behind a dictionary version.
3. **LUBM-1000's dictionary** (0.67 GB) is IRI-heavy, so item 2 matters more there, relatively, and item 1 less. It is
   to be measured with the same census.

Census output, verbatim, in `census-bsbm100.txt`.
