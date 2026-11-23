# The disk dictionary

A disk base maps term IDs to RDF terms, and back, through a dictionary that is read lazily: `open` reads only its
block directory, and blocks are read, checked and decoded on demand through a bounded cache. The code is
`crates/triplelight-storage/src/lazy_dictionary.rs` and `term_block_codec.rs`. Formats and the other base files are
in [persistence.md](persistence.md).

## Files

Current bases (`RDFSNAP4` and later) have one file, `dictionary.unified` (magic `RDFDID03`; `RDFDID02`, with fixed 16-byte entry framing, is still read), in ID order. An ID's top
four bits are its kind: 1 IRI, 2 blank node, 3 literal. The external builder assigns IDs so that, within a kind, ID
order is term-text order (the canonical N-Triples text). So one file in ID order is also in term order, kind by kind,
and serves both lookups. Older `RDFSNAP3` bases have two files of the same block format, `dictionary.term`
(`RDFDTM01`, term order) and `dictionary.id` (`RDFDID01`, ID order); `RDFSNAP2` bases have a legacy file (`RDFDIC01`)
that is loaded fully into memory at open.

**Blocks.** A block holds up to 256 entries and at most 16 MiB + 8 KiB, so the largest term the parsers accept (16 MiB)
always fits. Entries are front-coded: each stores its 64-bit ID, the length of the prefix it shares with the previous
entry's text, and the rest of its text. Decoding is strict: every length is bounded, the text must be valid UTF-8,
parse as an RDF term and re-serialize to exactly the same text, IDs must be non-zero, and the block must end exactly
at its last entry.

**Directory.** For each block, the directory keeps its last key (the last ID, or the last term text in a term-order
file), in the unified file also its last term text, and its offset, length and SHA-256. A checksum covers the header
and the whole directory, and the base's manifest records that identity. `open` validates the magic, sizes, strictly
increasing keys, and in the unified file that each block's kind agrees with its last term, and that blocks of one
kind are in term-text order. It doesn't read the blocks.

## Lookups

- **By ID** (`resolve`, `with_term`): binary-search the directory by ID, read that one block, and binary-search it.
  Batched lookups (`with_terms`) group the requested IDs by block, so each block is read once per batch.
- **By term** (`lookup`): the kind comes from the text's first byte (`<` IRI, `_` blank node, `"` literal).
  Binary-search that kind's run of blocks by their last term text, then search the one block.

Both paths read the same blocks and share cache entries. An unknown ID is an error. A term that isn't in the
dictionary is `Ok(None)`. A read, checksum or decode failure is returned as a storage error and reaches the query as
an error, never as a missing term. Callbacks get a reference into a decoded block that the lookup holds (an `Arc`)
until the callback returns, so eviction can't invalidate it.

## Cache

The dictionary's cache gets three of the base's cache shares (see [persistence.md](persistence.md#block-cache)) and is
split into 16 shards, each with a byte budget. Eviction is scan-resistant: when a shard is full, a miss is admitted
only one time in four, and room is made by sampling eight residents and evicting one not referenced since the last
sample. A one-pass scan larger than the cache therefore can't flush a hot set of blocks. Admission and sampling use a
fixed per-shard seed, so a single reader's cache decisions are reproducible. Cache hits, misses, reads, bytes and
evictions are counted per lookup path (trace slots 6, by term, and 7, by ID) when `TRIPLELIGHT_TRACE_BLOCK_CACHE` is
set.

Files must stay unchanged while a base is open: a cached block is served without rereading the file. The dictionary
doesn't use mmap, so a file truncated underneath an open base produces an I/O error on the next uncached read, not a
crash.

## Building and verifying

The external builder sorts terms with bounded memory and writes a legacy `dictionary` file in strictly increasing
term-text order. Text order groups the kinds into three runs (`"` literals, `<` IRIs, `_` blank nodes), so
`convert_unified` writes `dictionary.unified` by reading the runs back in kind order (1, 2, 3). It doesn't sort again,
and it refuses any term that breaks the order or disagrees with its kind. The legacy file is removed before the
manifest is written. An in-memory build assigns IDs in first-seen order, so it writes the two-file `RDFSNAP3` layout
instead (`convert_legacy`).

`verify` reads every block of the unified file once (`check_text_order`). Decoding each block already checks one kind
per block, strictly increasing IDs and, within a kind, strictly increasing term text that agrees with the kind. Those
checks make the ID-to-term mapping one-to-one, so no second pass is needed. For the two-file layout, `verify` checks
every term-order entry against the ID file, and that both hold the same number of entries.
