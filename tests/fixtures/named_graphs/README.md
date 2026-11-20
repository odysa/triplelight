Named-graph fixtures (task #36). `data.trig` plus one query per file; expected answers come from Jena TDB2
(`tdb2.tdbloader` + `tdb2.tdbquery`, where FROM / FROM NAMED select graphs of the loaded dataset) and are
compared with `tests/named_graphs.rs`. The `:name` triples only give graph names dictionary IDs in the Rust
test double; they live in the default graph here, so queries avoid matching them (all use `:p` or GRAPH).

`jena-tdb2-6.2.0.txt`: Jena TDB2 6.2.0 answers (rows sorted, header dropped). Agreement with `tests/named_graphs.rs`:
- agree: every query except the three below.
- `graph_empty_group`: Jena lists g1, g2 only — TDB2 cannot hold an empty named graph, so g3 does not exist there.
  TripleLight's catalog keeps empty graphs, so it also returns g3.
- `from_named_absent`: Jena treats `FROM NAMED :absent` as an (empty) named graph and binds ?g to it; TripleLight
  currently keeps only listed graphs present in the catalog. OPEN decision (SPARQL leaves unretrievable graphs to the
  implementation).
- `exists_per_graph` (+ `exists_per_graph_bgp`): Jena evaluates the EXISTS inside `GRAPH ?g` against all graphs
  (quad transform leaves ?g free; the empty-group form even returns one unbound row). SPARQL 1.1 §18.6 evaluates
  EXISTS in the active graph, which TripleLight does: only g2 contains `:a :p "2"`.
