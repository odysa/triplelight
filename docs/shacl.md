# SHACL Core validation

TripleLight validates an RDF graph against a SHACL Core shapes graph ([SHACL, W3C Recommendation 2017-07-20](https://www.w3.org/TR/2017/REC-shacl-20170720/)).
Both graphs are given as triples; the module (`src/shacl/`) uses only the platform-independent RDF types of `triplelight::rdf`, so the CLI and
the language bindings load the two graphs with the existing RDF readers and call one function.

```rust
use triplelight::shacl::validate;

let report = validate(&data_triples, &shapes_triples)?;   // Result<ValidationReport, ValidationError>
if !report.conforms {
    for r in &report.results { println!("{} {:?} {}", r.focus_node, r.severity, r.source_constraint_component); }
}
let as_rdf = report.to_triples();                          // a sh:ValidationReport graph
```

## What it covers, and what it does not

- **Covered:** SHACL Core: node and property shapes, the four target kinds (`sh:targetNode`, `sh:targetClass`, `sh:targetSubjectsOf`,
  `sh:targetObjectsOf`) and the implicit class target, all property path forms (predicate, inverse, sequence, alternative, `*`, `+`, `?`),
  `sh:deactivated`, `sh:severity` (including user-defined severity IRIs), `sh:message`, and every constraint component of the Core
  vocabulary: `sh:class`, `sh:datatype`, `sh:nodeKind`, counts, length, `sh:pattern` and `sh:flags`, `sh:languageIn`, `sh:uniqueLang`,
  the value-range and property-pair components, `sh:in`, `sh:hasValue`, `sh:closed` with `sh:ignoredProperties`, the logical components
  (`sh:not`, `sh:and`, `sh:or`, `sh:xone`), `sh:node`, `sh:property` and the qualified value shape counts.
- **Not covered:** SHACL-SPARQL (`sh:sparql`, SPARQL-based constraint components), SHACL Advanced Features, and the SHACL 1.2 drafts.
  Validation is over the two graphs as given: no RDFS or OWL entailment is applied to them. Class membership follows `rdfs:subClassOf`, as SHACL
  specifies, and nothing else.
- **Errors, not reports:** a shapes graph that breaks a SHACL rule (a malformed path or list, a bad parameter), a recursive shape reference
  (which SHACL leaves undefined), or an unsupported feature returns a `ValidationError` and no report. A data graph that violates the shapes is a
  report, not an error.

## Semantics worth knowing

- `conforms` is true when there are no validation results at all: a result of any severity makes the graph non-conforming.
- Value nodes of a property shape are a set (duplicates collapse), in the order they are reached.
- A shape's `sh:message` values replace a component's default message in every result of that shape.
- `sh:pattern` uses the Rust `regex` engine, not the XPath regular-expression dialect SHACL names. The two agree on common patterns and
  differ on a few features (for example look-around is not supported); a pattern the engine cannot compile is a malformed shape.

## Layers

```
evaluator   targets, value nodes, the recursion guard, results   (src/shacl/evaluator.rs)
   |  uses
shapes      the shapes graph: parsing, targets, severity         (shapes.rs)   constraints  the components (constraints.rs, constraints/)
   |                                                                |
path        parsing and evaluating property paths               graph, report   (the indexed graph; the result types)
   \________________________ error, vocab (leaves) ________________/
```

The constraint components do not depend on the evaluator: a component that refers to another shape (`sh:node`, `sh:property`, `sh:not`, ...)
receives a `check_node(shape, focus)` callback, so the layer boundary has no cycle (the repository's `deslop-please` gate enforces this).

## Conformance

`tests/shacl/w3c.rs` runs the W3C SHACL Core test suite (`data-shapes-test-suite/tests/core` of w3c/data-shapes) through `validate`. The test is
ignored by default; point `TL_SHACL_TESTS` at that directory of a checkout of revision `eacdef4528d66ee116f0ac9118d968c5717c7233` and run

```
TL_SHACL_TESTS=/path/to/data-shapes-test-suite/tests/core cargo test --test shacl -- --ignored --nocapture
```

Each expected result is compared by focus node, value, result path, severity, source constraint component and source shape (as a multiset),
and `sh:conforms` must match; messages are compared when the expected result has any. Cases declared not to pass are listed with a reason in
`tests/shacl/w3c-exclusions.txt`, and the run fails both on an unexpected failure and on an excluded case that now passes.
