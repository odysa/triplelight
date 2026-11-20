Named-graph fixtures (task #36). `data.trig` plus one query per file; expected answers come from Jena TDB2
(`tdb2.tdbloader` + `tdb2.tdbquery`, where FROM / FROM NAMED select graphs of the loaded dataset) and are
compared with `tests/named_graphs.rs`. The `:name` triples only give graph names dictionary IDs in the Rust
test double; they live in the default graph here, so queries avoid matching them (all use `:p` or GRAPH).

`jena-tdb2-6.2.0.txt`: Jena TDB2 6.2.0 answers (rows sorted, header dropped, blank node labels shown as `_:BNODE`). Agreement with `tests/named_graphs.rs`:
- agree: every query except the ones below.
- `graph_empty_group`: Jena lists g1, g2, g4, g5 only — TDB2 cannot hold an empty named graph, so g3 does not exist there.
  TripleLight's catalog keeps empty graphs, so it also returns g3.
- `from_absent`, `from_named_absent`: Jena treats a missing FROM / FROM NAMED source as an empty graph (and binds
  ?g to it). TripleLight's local-only dataset construction policy (decided in #36) refuses a source missing from the
  catalog with an Unsupported error; present-but-empty graphs are accepted. This is a policy choice SPARQL 1.1 §13.2
  permits (a service may refuse a dataset description), not a claim that Jena is non-conforming.
- `exists_per_graph` (+ `exists_per_graph_bgp`): OPEN, attribution pending review. Jena's quad form leaves ?g free
  inside the EXISTS (the empty-group form even returns one unbound row). TripleLight evaluates EXISTS in the active
  graph (our reading of SPARQL 1.1 §18.6), so only g2, which contains `:a :p "2"`, qualifies.
- `from_shared_bnode`: g4 and g5 share `_:x` (plus `:iri` in both). The RDF merge required by SPARQL FROM standardizes
  `_:x` apart, so the specification-derived answer is `:iri` only (Jianlin's controls, #36). Jena answers `:iri` and the
  shared blank node (a shared-ID union, not a merge). TripleLight refuses multi-graph FROM unless the store confirms the
  sources share no blank node (`DatasetView::shares_blank_nodes`): a scoped limitation, not a merge implementation.
- `graph_shared_bnode`: joining across two GRAPH patterns keeps the dataset's own identity (no merge); Jena and
  TripleLight both return `:iri` and the shared blank node.
