# Store sizes at the format freeze (#105, storage lane)

These are the sizes the paper cites, as Kaiming asked: one table, one binary, every file.

**How they were measured:**
- **Format:** `RDFIDX04` index blocks (tag-rotated columns) and the `RDFDID04` dictionary (lean framing, zstd literal
  blocks).
- **Binary:** main `9dfaa33c`, one release binary; its hash is in `binary.txt`.
- **Builds:** `triplelight build-disk` (four orders, the default) and `build-disk --orders=6`, from the same N-Triples
  inputs as every #105 measurement (`build.sh`).
- **What's counted:** each file's size in bytes, in `sizes.txt`. A store's total includes its MANIFEST.

| Dataset | Orders | spo | sop | pso | pos | osp | ops | dictionary | Total |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| LUBM-1000 | 4 | 960.3 MB | — | 365.0 MB | 321.3 MB | 304.5 MB | — | 154.9 MB | **2.11 GB** |
| LUBM-1000 | 6 | 960.3 MB | 960.3 MB | 365.0 MB | 321.3 MB | 304.5 MB | 302.4 MB | 154.9 MB | **3.37 GB** |
| BSBM-100M | 4 | 705.4 MB | — | 286.1 MB | 252.8 MB | 299.4 MB | — | 2,104.0 MB | **3.65 GB** |
| BSBM-100M | 6 | 705.4 MB | 705.4 MB | 286.1 MB | 252.8 MB | 299.4 MB | 268.2 MB | 2,104.0 MB | **4.62 GB** |

**Four orders against six:**
- LUBM-1000: **0.625×**, so 37.5% smaller.
- BSBM-100M: **0.789×**, so 21.1% smaller.

The two derived orders (SOP and OPS) are what four orders save. On LUBM, SOP is SPO's size, so it is the larger saving.

**Where these numbers came from:**
- `docs/codec-rotate-v1/`: SPO went to 0.64× its earlier size.
- `docs/dict-lean-framing-v1/`: the LUBM dictionary went to 0.37×.
- `docs/dict-zstd-literals-v1/`: the BSBM dictionary went to 0.49×.
