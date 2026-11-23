# RDF 1.2 storage design (task #101)

Status: design, agreed in (internal discussion). Implementation follows Kaiming's term model PR (#102 to #104 build on
the same model). Everything here sits behind the `rdf-12` Cargo feature of `triplelight-storage` until all four 1.2
tasks are in. We implement the W3C RDF 1.2 drafts of November 2026 (RDF 1.2 Concepts is a Candidate Recommendation,
draft of 2026-11-16; N-Triples, N-Quads and Turtle 1.2 are Working Drafts) and update when they change.

## What storage has to hold

RDF 1.2 adds two kinds of term to RDF 1.1:

- **Triple terms**, written `<<( s p o )>>`. A triple term appears only in **object** position, and its own object
  may be another triple term, so they nest. Two triple terms are the same term exactly when their subjects,
  predicates and objects are the same terms.
- **Directional language-tagged strings**, written `"x"@en--ltr` or `"x"@ar--rtl`, with datatype `rdf:dirLangString`.

The term model is oxrdf 0.3.5 with its `rdf-12` feature: `Term::Triple(Box<Triple>)` and `Literal` carrying
`(language, direction)`. Its `Display` text is the canonical N-Triples 1.2 form, and this design depends on that.

## Dictionary: a fourth ID kind

The dictionary keys every term by its canonical N-Triples text, and a term ID carries the term's kind in its top four
bits (`id >> 60`): 1 for IRIs, 2 for blank nodes, 3 for literals (`lazy_dictionary.rs`, `text_kind` and `id_kind`).
Inside a kind, ID order is term-text order, which term lookup relies on.

- **Triple terms get kind 4.** `text_kind` maps a text starting with `<<(` to 4. This is the one change to the kind
  rule: today any text starting with `<` is an IRI, and an IRI can never start with `<<`, because `<` isn't allowed
  inside an IRI. The builder's `match term` gains `Term::Triple(_) => 4`.
- **Directional literals stay kind 3.** Their text starts with `"` like every literal. The direction is part of the
  text, so `"x"@en` and `"x"@en--ltr` are different terms with different IDs, as RDF 1.2 requires.
- **Equality.** Equal triple terms have equal canonical text, so they get one ID, with no extra structure.
- **Blank nodes inside triple terms** use the same labels as top-level blank nodes in that store, so
  `<<( _:b <p> <o> )>>` and a top-level `_:b` refer to the same node, as in the RDF 1.2 abstract syntax.
- **Size.** A triple term's text includes its nested parts, so a deeply nested term can be long. The existing
  16 MiB term limit (`MAX_TERM`) applies to the whole text, the same as the parser's token limit.

## The `triple_terms` table

The executor has to match patterns inside triple terms, for example `?x <p> <<( ?a <q> ?b )>>`, on IDs and without
parsing text. Each base therefore gets one more file:

- **`triple_terms`**: rows `(term ID, s, p, o)` of four big-endian IDs, sorted by term ID, with one row per distinct
  triple term. It is stored with the same block codec and checksums as an order file (`Width<4>`, the codec the quad
  orders already use).
- **Parts are dictionary terms.** Every part, including a nested triple term and a term that appears only inside a
  triple term, is in the dictionary, so a row's part IDs always resolve. A nested term has its own row.
- **API.** `Snapshot::triple_term_parts(TermId) -> Option<[TermId; 3]>`: a binary search over the block directory,
  then one block read through the block cache. The reverse direction needs no index: build the triple term's
  canonical text from its parts' texts and look it up in the dictionary (`find_term`), which is the same path as
  any constant in a query.
- **Scans over all triple terms** (for example `?x <p> ?t` with a later `isTRIPLE(?t)`) need no new index. Kind 4 is
  a contiguous ID range, and the existing orders return the IDs, which the executor filters by kind.

### Building it

- **Parse phase.** When a parsed triple's object is a triple term, the shard pushes the triple term's text and,
  recursively, every nested part's text into its term sorter. It also writes one raw four-position row per
  triple-term occurrence into a `triple-term` raw file, (term, s, p, o), as term references to be mapped.
- **ID assignment.** These rows go through the same (row, position) to term-ID mapping as the quad rows. Then
  `build_index::<4, 32>` sorts them by term ID and drops duplicates, which writes `triple_terms`. That reuses the
  #90 fixed-width sorter. A store with no triple terms writes no rows and no file.
- **In-memory path and overlays.** `MemorySnapshot` and the overlay keep a `HashMap<TermId, [TermId; 3]>` filled
  at insert time. Compaction writes it out like a base build.
- **`verify`** checks that every kind-4 dictionary entry has exactly one row, that each row's text, rebuilt from its
  parts, is the entry's text, and that no kind-4 ID appears in subject or predicate position in any order.

## Format versions and old binaries

The goal is that no store written today changes, and that no old binary misreads a 1.2 store.

- **A base without any 1.2 term is byte-identical to today's**, in whichever format it is written now (RDFSNAP4 or
  5 for six orders, RDFSNAP6 for fewer). All existing evidence and receipts stay valid, and the identity gates
  (#88, #90, #91) keep their meaning.
- **A base with a triple term or a directional literal is written as `RDFSNAP7`.** It has the RDFSNAP6 header (order
  mask, named flag, file identities) plus a `triple_terms` identity, which is all zeros if the store has
  directional literals but no triple terms. Current binaries accept only RDFSNAP2 to RDFSNAP6, so they refuse
  RDFSNAP7 at open, before reading anything else.
- **WAL and `CURRENT`.** A WAL line holding a 1.2 term would look like a torn tail to a current binary, which would
  **truncate the WAL there and lose committed data**. That's the same hazard task #36 fenced for named graphs. So
  before the first WAL record that contains a 1.2 term, `CURRENT` is rewritten with a new fence prefix, `rdf12-1`
  (for example `rdf12-1 dataset-1 base-3`), written with the usual write-temp, fsync, rename and fsync-directory
  sequence. Current binaries parse `CURRENT` strictly and refuse it ("corrupt CURRENT") before they open the WAL.
  The fence is one-way, like `dataset-1`, and compaction keeps it.
- **New binaries** read every older format unchanged. The feature-off build of the new code refuses RDFSNAP7 and
  the `rdf12-1` fence with "store needs RDF 1.2 support" instead of "corrupt".

## Tests

- Round trip through the external build, the in-memory build, WAL replay and compaction for:
  - nested triple terms;
  - triple terms sharing blank nodes with the top level;
  - directional and plain literals with the same text and language;
  - a term that appears only inside a triple term.

  Each must give the same answers, the same `triple_term_parts` and a passing `verify`.
- Byte identity: on a 1.1-only input (the LUBM-100 #91 gate), the feature-on build writes the same files as main.
- Fences: a current-main binary refuses an RDFSNAP7 base and an `rdf12-1` `CURRENT` without touching the WAL. This
  is checked with the release binary from main, as the task #36 fence test does.
- Mutations of the kind rule (`<<(` classified as an IRI), of row deduplication and of the fence order must each
  fail a test.

## Out of scope here

- Parsing (#102).
- SPARQL 1.2 patterns, functions and `ORDER BY` placement of triple terms (#103).
- Results formats and bindings (#104).
- Statistics treat a triple-term ID like any other object ID. The planner needs no change until #103 measures a
  reason for one.
