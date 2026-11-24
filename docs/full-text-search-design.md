# Full-text search over RDF literals

**Design proposal.** Start with token matching over asserted string literals. Preserve
SPARQL's dataset boundaries and use the current RDF snapshot as the source of truth.

## Query surface

Use a TripleLight extension function and keep the RDF pattern explicit.

```sparql
PREFIX tl: <urn:triplelight:text:>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?s ?label WHERE {
  ?s rdfs:label ?label .
  FILTER(tl:matches(?label, "graph database"))
}
```

Match all analyzed query tokens, in any order. Apply Unicode
`toNFKC_Casefold` to both arguments, split at the default UAX #29 word
boundaries, and retain segments containing a Unicode letter or number. Use
one Unicode data version for normalization, segmentation and character
properties. Record it in the analyzer ID. Keep the policy locale-independent
and document that the default segmentation has limited search quality for
languages that need dictionary-based word breaking. Check the scalar analyzer
on native and `wasm32-unknown-unknown`, and update the package notices for
the implementation and Unicode data before shipping. [Unicode default algorithms](https://www.unicode.org/versions/Unicode17.0.0/core-spec/chapter-3/)
· [UAX #29 word boundaries](https://www.unicode.org/reports/tr29/#Word_Boundaries)

Index `xsd:string` and `rdf:langString`
lexical forms, retaining language and RDF term identity in the dictionary.
An unbound or non-string argument is an expression error. A query string
with no tokens yields false. Duplicate tokens do not affect matching.
Literal language constraints remain ordinary SPARQL filters. Phrase matching,
ranking, stemming, and result limits can follow once their semantics are specified.

An extension function keeps the binding pattern visible and gives the optimizer
a semantics-preserving candidate source. Jena demonstrates the trade-off with
a property function that generates bindings directly and supports ranking, graph
and language arguments. Reserve the Jena `text:query` property function
(`http://jena.apache.org/text#query`) for a later binding operator, starting
with a documented subset of its predicate and text arguments. Compare
supported queries with Fuseki before claiming compatibility. [SPARQL 1.1 extension functions](https://www.w3.org/TR/sparql11-query/#extensionFunctions)
· [Jena text query](https://jena.apache.org/documentation/query/text-query.html)

## Index and snapshot lifecycle

Use an optional inverted index over **literal term IDs**, with a sorted
token dictionary and delta-encoded sorted postings. Index each eligible
dictionary literal once. Intersect postings starting with the shortest list.
Use OSP with the literal ID as the prefix to retrieve subjects and predicates,
or use POS when the predicate is fixed. Evaluate the original BGP and test its bound literals if that produces fewer probes. The index answers
which *literals* match. The active graph supplies actual triple membership and
therefore subject bindings, graph visibility, and SPARQL multiplicity. A
deleted triple can leave a harmless dictionary entry. Graph filtering happens
before any SPARQL LIMIT. Bound filters, OPTIONAL, expression errors, and
entailment use the scalar function wherever index pushdown has no proof of
equivalence.

### Build and publish

Start with an explicit `triplelight text-index <dir>` command against a
pinned, immutable disk base. Walk dictionary entries in ID order on an
uncached path to preserve the query block cache. Stream variable-width
records keyed by (token, literal ID) into the external sorter used by
`external_build.rs`. Merge sorted runs, deduplicate adjacent identical
pairs, and stream delta-encoded postings. One common token can have a large
posting list, so write it in blocks and spool the token directory instead
of buffering all postings or all vocabulary in RAM.

Use a default 128 MiB working-buffer budget for the analyzer, sorter and
writer combined, adjustable for the build. Charge merge heads, run buffers
and the current decoded record to that budget. If one record cannot fit,
fail the build and leave scalar evaluation available. This is a buffer
budget. Also measure process RSS, including dictionary read buffers and
allocator overhead. For uncompressed run records totaling R bytes, plan
scratch for at most two run sets (2R), plus the candidate sidecar F,
the spooled token directory, and any retained sidecars. Merge passes delete their consumed inputs after
their outputs close. Record actual scratch high-water usage and final
index bytes on BSBM-100M. Size and time are workload-dependent.

Publish `<db>/text-<generation>.v1` outside `base-<n>`. Bind it to the
generation and SHA-256 of the exact base MANIFEST, which already identifies
the dictionary. The header also records the analyzer and postings codec.
Write a temporary file, fsync it, atomically rename it and fsync the
directory. Validate the header, bounds and integrity before using it.
Missing, invalid or stale sidecars use scalar evaluation. MANIFEST,
CURRENT and WAL retain their current formats.

Compaction can assign different IDs. Publish only for the generation
actually indexed. New readers fall back until the matching sidecar exists.
Old readers can retain their open sidecar. Cleanup must respect pinned
bases, including shared reader locks when multi-process readers land.
Keep old sidecars if safe cleanup cannot establish that the corresponding
generation is unpinned. Add an optional post-compaction build after the
explicit workflow has been validated, outside the durable compaction
commit. A sidecar failure leaves the RDF snapshot queryable.

Start with scalar evaluation for dirty overlays. If measurements justify
it, extend the optimization with base postings plus scalar checks on
overlay-only literal IDs. The union is complete only when both use the
same analyzer, and the graph still decides triple membership. In-memory
databases can use the same scalar function, with in-memory postings as a
later optimization.

This fits the existing separation between the immutable base and mutable
overlay in [store.rs](../crates/triplelight-storage/src/store.rs). Model
publication and invalidation on [the statistics sidecar](optimizer.md),
while binding text IDs to the matching base. The release gate
should compare the same query with and without pushdown.

## Acceptance before implementation

- Establish exact equality of answers, bindings, multiplicity, and ordered
  LIMIT membership against scalar evaluation. Cover duplicate literals across
  graphs, FROM and FROM NAMED, tagged strings, blank-node subjects, Unicode,
  empty tokens and filter errors, including OPTIONAL.
- Exercise the same query on a pinned old reader and a new reader after
  inserts, deletes, graph clears, reopens, and compaction. Inject missing,
  truncated, and wrong-dictionary sidecars and check scalar fallback.
- Measure build time, sidecar bytes, peak memory, and query latency against
  scalar evaluation on short and long strings. Vary term selectivity and
  graph/predicate selectivity, with both warm and cold file cache. Report
  actual RDF candidate triples and verify answers before comparing timings.
  Adopt pushdown only where the measurements justify its maintenance cost.
