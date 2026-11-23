# Tag-rotated block columns: `RDFIDX04` / `RDFQIX02` (#105, storage lane)

**The problem:** a block stores every column except the first relative to its block minimum. An SPO block's object
column mixes term kinds: IRIs (tag 1 in an ID's top four bits), literals (tag 3) and inline numbers (tag 0xf). Its
span is about 2^62, so it takes 61 bits per value. The `block_widths` census measured this on LUBM-1000 (#105): 61.0
bits on average, with 521,774 of 521,930 SPO blocks mixing tags in that column.

**The change:** in the new formats, a column other than the first may store `id.rotate_left(4)`, which moves the tag
to the low bits. The encoder chooses this per block and column, only when it is narrower, and flags it in the top bit
of the column's width byte. Old `RDFIDX03`/`RDFQIX01` files stay readable. Older binaries refuse the new files by
their magic. The rotation is undone in a separate pass after unpacking, so columns that never rotate decode exactly
as before.

**Setup:**
- Laptop, one session.
- `scan_bench --keys 500 --passes 2`, ABBA: main `275793ff` on its store / this branch on its store / this branch /
  main. The stores are LUBM-1000 four-order bases built from the same input (133,613,894 triples).
- Raw output is in `r*.jsonl`, and the binaries' hashes are in `binaries.txt`.
- The row checksums of every phase are identical across all four runs, so the two stores hold the same IDs in the
  same order.

**Sizes, in bytes (`sizes.txt`):**

| File | main | rotated | ratio |
| --- | ---: | ---: | ---: |
| spo | 1,500,252,416 | 960,276,593 | 0.64 |
| pso | 365,033,604 | 365,025,028 | 1.00 |
| pos | 321,319,972 | 321,311,108 | 1.00 |
| osp | 304,533,924 | 304,533,924 | 1.00 |
| dictionary | 672,136,499 | 672,136,499 | 1.00 |
| **store total** | 3,163,276,415 | 2,623,283,152 | **0.83** |

**LUBM-1000, six orders on the new codec** (`lubm-six-sizes.txt`, `build-disk --orders=6`, same binary): spo
960.3 MB, sop 960.3 MB, pso 365.0 MB, pos 321.3 MB, osp 304.5 MB, ops 302.4 MB, dictionary 672.1 MB, 3.89 GB in
total. The four-order store (2.62 GB, above) is 0.675× of it, 32.5% smaller. That replaces the 36% measured with the
earlier codec, where SPO's larger blocks made the two extra orders count for more.

**BSBM-100M** on the new codec (`bsbm-sizes.txt`; the same build binary as the LUBM store, `build-disk` and
`build-disk --orders=6`):

| Store | spo | pso | pos | osp | sop | ops | dictionary | total |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| four orders | 705.4 MB | 286.1 MB | 252.8 MB | 299.4 MB | | | 4,620.2 MB | 6.16 GB |
| six orders | 705.4 MB | 286.1 MB | 252.8 MB | 299.4 MB | 705.4 MB | 268.2 MB | 4,620.2 MB | 7.14 GB |

The four-order store is 0.86× the six-order one. The dictionary is three quarters of either.

**Full scans** (seconds per pass, main r1/r4 against rotated r2/r3; main against main is within 1% except PSO pass
0, at 3.5%):

| Order | main | rotated | ratio |
| --- | ---: | ---: | ---: |
| SPO | 4.50 / 4.47 | 3.96 / 4.05 | 0.89 |
| SOP (derived from SPO) | 5.84 / 5.83 | 5.36 / 5.37 | 0.92 |
| PSO, POS, OSP, OPS | | | 0.98–1.01 |

Store open and the probe phases are unchanged. A first version undid the rotation inside the unpacking loop, which
cost 2–3% on columns that never rotate. Its runs are in `v1/`, not filed here. The separate pass removes that cost.

**Next, measured separately with the census first:** a per-block local dictionary for low-cardinality columns. SPO's
predicate column still takes 25 bits for typically 10–20 distinct predicates per block. SPO's object column is still
at about 29 bits after rotation, and frame of reference per tag group could narrow it further.
