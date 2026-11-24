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

Match all analyzed query tokens, in any order. Define one versioned Unicode
tokenizer and case-folding policy. Index `xsd:string` and `rdf:langString`
lexical forms, retaining language and RDF term identity in the dictionary.
An unbound or non-string argument is an expression error. A query string
with no tokens yields false. Duplicate tokens do not affect matching.
Literal language constraints remain ordinary SPARQL filters. Phrase matching,
ranking, stemming, and result limits can follow once their semantics are specified.

An extension function keeps the binding pattern visible and gives the optimizer
a semantics-preserving candidate source. Jena demonstrates the trade-off with
a property function that generates bindings directly and supports ranking, graph
and language arguments. TripleLight can add a binding operator when those
capabilities justify an additional query surface. [SPARQL 1.1 extension functions](https://www.w3.org/TR/sparql11-query/#extensionFunctions)
· [Jena text query](https://jena.apache.org/documentation/query/text-query.html)

## Index and snapshot lifecycle

Use an optional inverted index over **literal term IDs**, with a sorted
token dictionary and delta-encoded sorted postings. Index each eligible
dictionary literal once. Intersect postings starting with the shortest list.
Then seek the relevant RDF triples through POS/OSP, or evaluate the original
BGP and test its bound literals if that produces fewer probes. The index answers
which *literals* match. The active graph supplies actual triple membership and
therefore subject bindings, graph visibility, and SPARQL multiplicity. A
deleted triple can leave a harmless dictionary entry. Graph filtering happens
before any SPARQL LIMIT. Bound filters, OPTIONAL, expression errors, and
entailment use the scalar function wherever index pushdown has no proof of
equivalence.

Keep the sidecar in a separately versioned cache directory, outside
`base-<n>`. Leave the store MANIFEST, CURRENT, and WAL bytes untouched.
The file header records analyzer version, postings codec, and the hash of
the immutable dictionary identity for which IDs were generated. Publish a
complete immutable file via temporary write, fsync, atomic rename and parent
directory fsync. Validate header, size bounds and integrity before using it.
A missing, invalid, or stale index selects scalar evaluation and can be
rebuilt. Use the base dictionary only when its identity matches the graph
snapshot, and independently evaluate literals introduced in the overlay.
Initially disable index pushdown for dirty overlays if obtaining a complete
set of overlay-only literals would require a new storage interface.

Compaction can assign different IDs. Build against a pinned generation and
allow readers to keep its sidecar open for the lifetime of that snapshot.
Readers on the new generation fall back until its own sidecar is published.
A crash before or after cache publication affects acceleration only. Old
binaries continue opening the same store. In-memory databases can build the
same postings in memory as an optional optimization.

This fits the existing separation between the immutable base and mutable
overlay in [store.rs](../crates/triplelight-storage/src/store.rs). Model
publication and invalidation on [the statistics sidecar](optimizer.md),
while keying the text cache to the dictionary identity. The release gate
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
