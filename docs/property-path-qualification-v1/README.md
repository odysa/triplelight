# Independent property-path qualification controls

This finite suite checks 45 SPARQL path queries against Apache Jena. It is a correctness harness, not a performance benchmark or a complete SPARQL conformance certification. Default graphs only; returned blank-node labels and cross-result isomorphism are deliberately outside scope. One fixture does traverse a blank node as a hidden intermediate.

The fixture combines a cyclic diamond, a self-loop, disconnected terms, predicate-only IRIs, language-tagged literals and two distinct typed lexical forms. A repeated source triple checks RDF graph set semantics. Separate empty and blank-node-intermediate fixtures cover zero-length and hidden-node cases. SELECT comparisons preserve exact RDF terms, unbound states and duplicate rows. The explicitly ordered LIMIT/OFFSET case additionally compares the complete sequence. ASK results are compared as booleans. There are no numeric tolerances or COUNT datatype relaxations.

SPARQL sequence and alternative expressions can preserve multiplicities from their join/union expansions; arbitrary-length reachability has different deduplication rules. Zero-length node enumeration uses subjects and objects, not predicates alone. These distinctions motivate the suite, following the [property-path definitions and examples](https://www.w3.org/TR/sparql11-query/#propertypaths) and [formal evaluation](https://www.w3.org/TR/sparql11-query/#defn_evalPP). Several manually specified row/boolean anchors additionally check the generated oracle. Jena output is retained even if an anchor fails; no failed attempt is replaced silently.

Prepare references (fresh output directory required):

```sh
python3 qualify_paths.py --suite suite.json --jena-home /path/to/apache-jena-6.2.0 --output reference-attempt1
```

Qualify a fixed CLI binary against those reference hashes:

```sh
python3 qualify_paths.py --suite suite.json --references reference-attempt1 --candidate /path/to/triplelight --candidate-source FULL_COMMIT --output candidate-attempt1
```

The candidate CLI protocol is `BINARY query DATAFILE QUERY_TEXT`, returning SPARQL JSON. Every process has a timeout and retained stdout, stderr, command and exit status. Missing, malformed, rejected or timed-out results are failures, never empty answers. A failing qualification exits nonzero. Each result directory records suite, query, fixture, reference and binary hashes. Reusing references requires the same suite hash. Candidate source identity is an explicit label supplied by the operator; the binary SHA is independently calculated, so build provenance must be provided separately.

Run the adversarial comparison tests with `python3 -m unittest -v test_comparison.py`. They check multiplicities, row ordering, projection, unbound terms, exact numeric lexical identity, RDF string identity and ASK/SELECT separation.

On the shared benchmark machine, these commands require current machine-lock ownership before execution. Retain both successful and failed attempts and release only the lock you own after child processes have exited. This directory contains no engine changes and requires no Docker service.
