# `sha2` 0.11: hardware SHA-256 on ARM64 (#105, storage lane)

`sha2` 0.10 uses the ARMv8 SHA instructions only with its `asm` feature, which TripleLight does not enable. Every
SHA-256 on Apple Silicon or Graviton therefore ran the software implementation. That covers the per-block checksum on
every block-cache miss, and the store identity at open. `sha2` 0.11 detects the instructions at runtime. On x86_64,
0.10 already detects the SHA extensions at runtime, so the rental machine is not affected. `sha1` and `md-5` move to
0.11 with it, so the SPARQL `MD5`/`SHA1` functions share one `digest` version. There is no format change, and every
digest is the same.

**Setup:**
- Laptop (macOS, Apple Silicon), one session, ABBA: main `8b1019de` / bump / bump / main.
- `scan_bench` (`crates/triplelight-storage/examples/scan_bench.rs`) on a fresh four-order LUBM-1000 store
  (133,613,894 triples), `--keys 5000 --passes 1`.
- Raw output is in `r*.jsonl`, and the driver is `run.sh`. The binaries' hashes are recorded in the PR.
- The checksum of the rows read is identical in all four runs for every phase. The main/main noise is at most 6%
  per phase, and at most 2% on the full scans.

| Phase | main (s, r1 / r4) | 0.11 (s, r2 / r3) | 0.11 / main |
| --- | ---: | ---: | ---: |
| open | 0.491 / 0.469 | 0.162 / 0.116 | 0.29 |
| full scan SPO | 8.165 / 8.068 | 4.503 / 4.506 | 0.55 |
| full scan SOP (derived) | 9.499 / 9.430 | 5.828 / 5.852 | 0.62 |
| full scan PSO | 3.738 / 3.696 | 2.853 / 2.847 | 0.77 |
| full scan POS | 3.501 / 3.466 | 2.728 / 2.722 | 0.78 |
| full scan OSP | 3.399 / 3.342 | 2.660 / 2.642 | 0.79 |
| full scan OPS (derived) | 4.826 / 4.768 | 4.115 / 4.096 | 0.86 |
| one-column probes, kept orders | 0.024–0.137 | 0.015–0.137 | 0.64–1.00 |
| seeks, kept orders | 0.007–0.046 | 0.007–0.046 | 0.98–1.00 |

How to read the table:
- **Full scans:** these miss the block cache on every block, so the SHA-256 per block is a large share of their time.
  Probes and seeks mostly hit the cache, so they barely move.
- **SPO:** it gains the most because its blocks are 4.7× larger (1.4 GB against about 300 MB for PSO, POS and OSP),
  so it hashes the most bytes. That size is a separate finding, below.

**Two findings outside this change, recorded for #105 (their own PRs):**
- **OPS one-column probes and seeks are slow:** about 21 ms per probe (106 s for 5,000) and 63 s for 5,000 seeks.
  OPS is derived, so each probe regroups the object's whole OSP group before its first batch. That's large for
  class objects. It is the case usage-driven orders (`docs/adaptive-orders-design.md`) address.
- **SPO is 4.7× the size of the other orders.** The hypothesis, still to be measured: an SPO block's object column
  mixes term tags (IRI = 1, literal = 3, inline = 0xf in the top four bits). The column is encoded relative to its
  block minimum, so it needs about 62 bits per value.
