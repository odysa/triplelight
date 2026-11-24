# RDFS answers against Jena, LUBM-1 (#108)

Build the CLI with `cargo build --release --features rdfs` to reproduce this check.

`bench/rdfs/check.sh` runs each of the 14 original LUBM queries both ways and compares the answers as multisets:
TripleLight with `--entailment=rdfs` over the unclosed graph, and Jena 6.2.0 `sparql` over the closure Jena's
`infer --rdfs` computes. Both sides use the same rules: subclass and subproperty transitivity, type and property
propagation, domain and range, with no axiomatic triples and no range types on literals. The input is the univ-bench
ontology plus University0 of the UBA 1.7 data (seed 0): 103,410 triples, closed by Jena to 126,738 distinct triples.

All 14 queries give the same answers (`check.txt`).

`infer` is riot's streaming RDFS, not Jena's rule-based `RDFSRuleReasoner`. I checked the closure directly: it has no
`rdfs:Resource`, no `rdf:type rdfs:Class`, no `rdfs:Literal` or container-membership triples, and no type triples with
a literal subject. So the agreement holds rule for rule, and doesn't depend on LUBM's queries missing those triples.

Six queries are empty under RDFS, and the same on both sides, because what they need is OWL:
- **Q6, Q8 and Q10** count `Student`. In univ-bench a graduate student is a `Student` only through an OWL class
  intersection, so RDFS finds the undergraduates alone (Q10 asks about a graduate course).
- **Q11** needs `subOrganizationOf` to be transitive.
- **Q12** needs `Chair`, an OWL restriction.
- **Q13** needs `hasAlumnus`, an inverse property.
- **Q2** is empty on LUBM-1 even with full reasoning.

So these are RDFS answers, not the LUBM reference answers.

The ontology is `http://swat.cse.lehigh.edu/onto/univ-bench.owl`, SHA-256
`e6eca926fcb7d6c7925ea0c48f7c5d79abc2818f89e7432fd16561aafeb3f67a`.
