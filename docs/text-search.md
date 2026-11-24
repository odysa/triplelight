# Match words in RDF literals

Use the TripleLight extension function `tl:matches` in a SPARQL expression.
The RDF pattern supplies the candidates, and the function tests the lexical
form of each candidate.

```sparql
PREFIX tl: <urn:triplelight:text:>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?item ?label WHERE {
  ?item rdfs:label ?label .
  FILTER(tl:matches(?label, "graph database"))
}
```

Both arguments accept plain strings and language-tagged strings. Language
tags remain available through ordinary SPARQL expressions. The function
compares whole words after Unicode 17.0.0 NFKC case folding and default
word segmentation. Every query word must appear. Word order and repeated
query words have no effect. A search string with no words evaluates to
false. Other argument types, unbound arguments, and an incorrect argument
count produce a SPARQL expression error.

Default word boundaries can give coarse results in languages that depend
on dictionary-based segmentation.

## Build an optional index

To build the sidecar for an existing disk store, stop any server using the
store and run:

```sh
triplelight text-index /path/to/store
```

The builder requires the unified disk dictionary. It visits the immutable
base, including its named-graph dictionary entries, and writes
`text-<generation>.v1` outside the base directory. The active base and WAL
remain authoritative. Reopen the store after building the sidecar. The
first eligible text filter checks its generation, manifest hash, analyzer
version, and payload hash. Later filters on that snapshot reuse the result
of the check. An open snapshot that already cached a missing index keeps
using scalar matching.
A simple positive filter with a constant search string can check eligible
stored literal IDs against its postings. Other expressions, dirty
overlays, large posting lists, and missing or invalid sidecars use scalar
matching. The RDF graph still supplies the candidate bindings.

The default 128 MiB build budget allocates 96 MiB to the external sorter.
Change the budget with `--build-memory-mb=N`. The cap on each intermediate
and normalized string is one-sixteenth of the budget, up to 8 MiB. Dictionary decoding, allocator overhead,
and I/O buffers contribute to process memory separately. An input larger
than the analyzer can handle aborts the build. Temporary files are cleaned
up on ordinary error paths. The completed file is synced and published
atomically. The store removes retired sidecar paths after a successful
compaction or reopen. A pinned snapshot that already opened a retired
sidecar keeps its file descriptor. A pinned snapshot whose first lookup
comes after the file is removed can still use scalar matching.
