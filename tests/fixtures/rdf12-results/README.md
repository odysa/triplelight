# RDF 1.2 / SPARQL 1.2 results fixtures (task #104)

Expected serializations copied from the examples in the W3C drafts, as fetched on 2026-11-22. They are drafts and may change; each file names its
source version. Use them as fixtures for the result writers, and update them together with the pinned draft dates in the docs.

| File | Source (W3C Working Draft) |
|---|---|
| `triple-terms.srj` | SPARQL 1.2 Query Results JSON Format, 2026-11-19, the triple-term example |
| `head-version.fragment.json` | same document, the `head` structure with `link` and `version` (a fragment, not a complete result) |
| `triple-terms.csv`, `directional-literal.csv` | SPARQL 1.2 Query Results CSV and TSV Formats, 2026-11-16 |
| `triple-terms.tsv`, `directional-literal.tsv` | same document (tab-separated) |

Notes from the drafts: JSON writes a triple term as `{"type":"triple","value":{"subject":S,"predicate":P,"object":O}}` and a directional literal with
`"its:dir"` next to `"xml:lang"`; the 1.2 version is announced by the media-type parameter `version=1.2` or a `"version"` member of `head`.
TSV uses Turtle-style terms (`<<( s p o )>>`, `"x"@en--ltr`); CSV prints lexical forms without type markers.
`directional-literal.*` are the spec's whole mixed-term example (IRIs, blank nodes, missing cells, language tag, and the directional literal `"String-with-lang-dir"@en--ltr`). Check the exact CSV cell text against the spec
when the writer is implemented; the CSV example in the draft prints the lexical value only.
