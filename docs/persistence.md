# Persistence and disk formats

This page describes how TripleLight stores data on disk: the commands, the store directory and its write-ahead log,
the immutable base formats, how bases are built, opened and verified, and the block cache. The code is in
`crates/triplelight-storage` (`store.rs`, `disk_snapshot.rs`, `disk_index.rs`, `lazy_dictionary.rs`,
`external_build.rs`).

## Commands

```sh
# Build a base directory from N-Triples (fails if the directory exists).
triplelight build-disk data.nt ./snapshot --build-external
# N-Triples only: keep four of the six triple orders (an RDFSNAP6 base; see "Kept orders" below).
triplelight build-disk data.nt ./snapshot4 --orders=4
# Query a base, or check every block and cross-index invariant (also prints each file's size).
triplelight query ./snapshot 'SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }' --disk
triplelight verify ./snapshot
# Serve a durable store: a base plus a write-ahead log (needs `--features server`).
triplelight serve-store ./store 7878 --disk --init data.nt
```

## The store directory

A durable store (`Store`, used by `serve-store` and the embedded APIs) is a directory:

| Entry | Contents |
| --- | --- |
| `CURRENT` | one line naming the active base: `base-<n>`, or `dataset-1 base-<n>` once named graphs are in use |
| `base-<n>/` | an immutable base, never modified after it is written |
| `wal.log` | the change sets committed since that base was written |
| `LOCK` | held by the one process that has the store open |

**Commits.** A commit appends its change set to the WAL as one record, fsyncs, and then publishes a new read
snapshot. Concurrent commits share one fsync (group commit). Readers hold a consistent snapshot (the base plus the
changes up to some commit), which later commits never change. A commit refuses to insert a term larger than 16 MiB
(the parsers' token limit); removals are always accepted, so a term that an older binary let in can still be deleted.

**WAL records.** One line per change, then a line `C`. A change is `+ <triple> .` or `- <triple> .` (the default
graph, N-Triples), `+ <triple> <graph> .` or `- <triple> <graph> .` (a named graph, N-Quads), `G+ <graph>` (a graph
joins the catalog) or `G- <graph>` (it leaves, with its triples). A record holds the net effect of one update, so
replaying it on a state that already contains it changes nothing. Recovery replays complete records and truncates a
torn tail.

**Named-graph fence.** A binary that predates named graphs would read the first named-graph line as a torn tail and
truncate the log there. So before the first record that touches a named graph, `CURRENT` is rewritten as
`dataset-1 base-<n>`. Older binaries accept only `base-<n>`, so they refuse such a store before reading its WAL. The
change is one-way.

**Compaction** writes the base plus the WAL as a new base `base-<n+1>`, opens it to validate it, atomically replaces
`CURRENT` (write a temporary file, fsync, rename, fsync the directory) and then empties the WAL. A crash leaves either
the old base with the full WAL or the new base with a WAL whose records re-apply as no-ops. Base directories that
`CURRENT` does not name are deleted at open. Readers that still hold the old base keep it until they drop it, so old
and new bases can exist on disk at the same time. Compaction keeps the order set of the base it replaces.

## Base formats

A base directory holds a `MANIFEST`, written last, and the files it names. The manifest starts with an 8-byte
version and lists a 32-byte SHA-256 identity for each file, which `open` checks. For a block-indexed file (the triple
and quad orders and the `RDFSNAP3`+ dictionaries) the identity covers its header and block directory; for the
`RDFSNAP2` legacy dictionary it is a SHA-256 of the whole file. The graph catalog ends with a SHA-256 of everything
before it, and the manifest holds that same checksum.

| Manifest | Dictionary | Triple orders | Named graphs |
| --- | --- | --- | --- |
| `RDFSNAP2` | one legacy file, loaded fully at open | six | no |
| `RDFSNAP3` | `dictionary.term` and `dictionary.id` | six | no |
| `RDFSNAP4` | `dictionary.unified` | six | no |
| `RDFSNAP5` | `dictionary.unified` | six | six quad orders and `graphs` |
| `RDFSNAP6` | `dictionary.unified` | the kept set (at least four) | if its named-graph flag is set: the kept quad orders and `graphs` |

- **Triple orders** are the files `spo`, `sop`, `pso`, `pos`, `osp`, `ops` (format `RDFIDX03`; `RDFIDX02` is still
  read). Each holds the triples sorted by its key, in blocks of 256 rows. A block is compressed (the first column is
  delta-coded, the others are stored relative to the block minimum, and each column is bit-packed to its widest
  value) and has its own SHA-256. The block directory, holding each block's last key, SHA-256 and byte length, is read
  at open.
- **Quad orders** (`RDFSNAP5`/`6`) are the files `gspo` … `gops` (format `RDFQIX01`): the same layout with four IDs
  per row, graph first. The **catalog** `graphs` (`RDFGRA01`) lists every named graph's ID and triple count, empty
  graphs included.
- **The unified dictionary** (`RDFDID02`) is one front-coded file in ID order. IDs carry their kind in the top four
  bits (1 IRI, 2 blank node, 3 literal). Within a kind, ID order is term-text order, so the same blocks serve both
  lookups: by ID, by binary search on IDs; by term, by taking the kind from the text's first byte (`<`, `_` or `"`)
  and binary-searching that kind's blocks by their last term text. Both paths share cache entries. See
  [lazy-dictionary-design.md](lazy-dictionary-design.md).
- **Upgrades and downgrades.** Older bases stay readable and are rewritten in the current format at their next
  compaction. There is no downgrade: a binary refuses a manifest version newer than it knows.

### Kept orders (`RDFSNAP6`)

On the 54 LUBM, WatDiv and BSBM benchmark queries, SPO, PSO and POS are used by most queries, OSP by one, and SOP and
OPS by none, yet SOP is as large as SPO (task #73). Every one of those queries binds its predicate. An `RDFSNAP6` base can therefore keep a subset of the orders. After
the magic come two bytes, the kept orders (bit *i* is `Perm::ALL[i]`, in the order SPO, SOP, PSO, POS, OSP, OPS) and
1 if the base holds named graphs (else 0), then the identities of only the kept files. Every base keeps SPO, PSO,
POS and OSP; `open` refuses any other set.

When SOP or OPS is absent, the base answers it with the same rows in the same order (`derived_order.rs`):

- With the first two key columns bound, SOP `[s,o,..]` is read from OSP `[o,s,..]` and OPS `[o,p,..]` from POS
  `[p,o,..]`. The third column is already sorted there, so this is a lookup in a kept order with no group sort.
- Otherwise rows come from the kept order with the same first column (SPO for SOP, OSP for OPS), one first-column group
  at a time, and each group is sorted. A seek within one group reads the group once and then binary-searches it.
  Only a scan with nothing bound reads and sorts the whole table, one group at a time.

The same rules apply to the quad orders `gsop` and `gops`. `build-disk --orders=4` writes such a base from N-Triples
(a Turtle input still goes through the in-memory path and gets all six orders); without the flag, builds write all six
orders in the `RDFSNAP4`/`RDFSNAP5` layout. A base keeps its set across compactions; only
a new build changes it.

## Building a base

`build-disk` (and a store's first import) chooses a strategy from the input size. If three times the N-Triples file
size fits the build budget, an in-memory loader parses it and writes the base; otherwise an external-sort builder
writes it from sorted scratch runs. The budget is advisory (not a peak-memory limit): `--build-memory-mb=N`, by default
half of physical memory, or 256 MiB if that cannot be read. `--build-external` forces the external builder, and
`--orders=4` implies it. Compaction always uses the external builder. Turtle input uses the in-memory loader. The
external builder writes its scratch files inside the output directory and deletes them before writing the manifest.

The in-memory path writes `RDFSNAP3` with all six orders. Its IDs are assigned in first-seen order, and the unified
dictionary needs term-text order within each kind, which the external builder produces. Such a base becomes
`RDFSNAP4` (or `RDFSNAP5`) at its first compaction.

A failed build can leave an incomplete directory, but it has no `MANIFEST`, so nothing opens it as a base.

## Opening and verifying

`open` reads the manifest and every file's header and block directory, and checks versions, bounds and identities.
Data blocks are read later, on demand, with positional reads, and each block's checksum is checked before use; a
failure is returned as a storage error. Keep base files immutable while open: a block already in the cache is
served even if the file changes underneath it. The checksums detect accidental corruption; they do not authenticate
files.

`triplelight verify` (`DiskSnapshot::verify`) is exhaustive and is not done at open. It re-reads every block of every
kept order; checks that every order holds the same triples and that every ID resolves in the dictionary; checks the
dictionary's ordering (one pass for the unified file); and for named graphs checks every quad order and the catalog's
counts. At BSBM-100M it takes 8 min 41 s (#55). It then prints each file in the base with its size in bytes.

## Block cache

Decoded blocks are cached in memory. The cache target is one quarter of physical memory (macOS `hw.memsize`, Linux
`MemTotal`), or 256 MiB if that cannot be read; `--cache-mb=N` overrides it on the command line. The target is per
base, not per process, and is divided into shares: each triple order gets one share and the dictionary three, so a
base has 9 shares, or 15 with the six quad orders. An absent `RDFSNAP6` order's share stays unused. Memory is
allocated as blocks are cached. At least one block per cache shard, allocator overhead and metadata fall outside the
target.

A fresh process starts with an empty block cache, but the operating system's page cache may still hold the files.
