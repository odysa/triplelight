# W3C SPARQL 1.1 query test suite

## Latest: main `9a2f49b` (exact decimals)

pass 281, fail 6, unsupported 44, skipped 19 / 3. `agg-sum-01` now passes. Remaining: 5 lexical canonical-form
cases (`ceil01`, `floor01`, `round01`, `cast-decimal`, `cast-string`) and `values-construct-group-by` (parser).
Details: `9a2f49b.json`.

## Previous: main `99ad969` (group scoping, LIMIT subqueries, IRI() BASE fixed)

| Outcome | Tests |
|---|---:|
| pass | 280 (97 syntax, 183 evaluation) |
| fail | 7 |
| unsupported (explicit error) | 44 (24 property paths, 20 functions) |
| skipped: named graphs / harness | 19 / 3 |

Remaining failures: 3 lexical form only (`ceil01`, `floor01`, `round01`: `"2.0"` vs `"2"`); 4 real bugs —
`agg-sum-01` (decimal SUM uses f64), `cast-decimal`, `cast-string`, and `values-construct-group-by` (valid query
rejected by the parser). Details: `99ad969.json`.

## Previous: main `ef7a778` (string-function semantics fixed)

| Outcome | Tests |
|---|---:|
| pass | 276 (97 syntax, 179 evaluation) |
| fail | 11 |
| unsupported (explicit error) | 44 (24 property paths, 20 functions) |
| skipped: named graphs / harness | 19 / 3 |

Remaining failures: 3 are lexical form only (`ceil01`, `floor01`, `round01`: rdfdb writes integral decimals as
`"2.0"`, like Jena; the suite expects `"2"`); 8 are real bugs — `agg-sum-01` (decimal SUM uses f64), `iri01`
(IRI() ignores BASE), `cast-decimal`, `cast-string`, `bind07`, `bind10`, `subquery11`, and
`values-construct-group-by` (valid query rejected by the parser). Details: `ef7a778.json`.

## Earlier: main `f7cb4ef`

Suite: w3c/rdf-tests `main` (sparql/sparql11, `manifest-sparql11-query.ttl`), run with `bench/w3c_sparql11.py`
through the `rdfdb query` CLI on 2026-11-18.

| Outcome | Tests |
|---|---:|
| pass | 266 (all 97 syntax tests; 169 evaluation tests) |
| fail | 21 |
| unsupported (explicit `Unsupported` error) | 44 — 24 property paths, 20 functions (hashes, NOW, UUID, …) |
| skipped: needs named graphs | 19 |
| skipped: harness could not load the test data (TriG-style named-graph data) | 3 |

Of the 234 evaluation tests rdfdb attempted or rejected, 169 pass (72%); of those it attempted (pass + fail), 89%.

Failures (21): `agg-sum-01`, `bind07`, `bind10`, `values-construct-group-by` (valid query rejected by the
parser), `cast-decimal`, `cast-string`, `strdt01`, `strdt03-rdf11`, `strlang01`, `strlang03-rdf11`, `ceil01`,
`floor01`, `round01`, `concat02`, `iri01`, `strbefore01a`, `strbefore02`, `strafter01a`, `strafter02`,
`replace01`, `subquery11`. Per-test details are in `f7cb4ef.json`.

Harness limits (so "pass" is not stronger than this): SELECT results are compared as multisets of terms with
blank-node labels erased (not full blank-node isomorphism); ORDER BY sequences are not checked here (the BI
benchmark checks them separately); CONSTRUCT results use graph isomorphism; tests needing named graphs, CSV/TSV
result formats or entailment are not run. Each query is prefixed with `BASE <file://query-path>`.
