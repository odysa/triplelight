# Independent property-path qualification of TripleLight 52fd1d71

The fixed candidate passes the declared behavior of all 45 original controls. Forty-three match Apache Jena 6.2 exactly; two constant/constant zero-length ASK cases disagree with Jena and satisfy the manual true anchors declared before execution. The distinction remains explicit. Seven exploratory bound-endpoint controls and two original W3C tests were also retained. No implementation defect was identified within this tested path scope. The added functions in the same commit were not independently qualified by this run.

## Results and limits

| Group | Cases | Exact Jena matches | Assessment |
|---|---:|---:|---|
| Original frozen controls | 45 | 43 | All 45 satisfy their declared contract; 2 predeclared boolean anchors disagree with Jena |
| Exploratory bound-endpoint supplement | 7 | 2 | 5 Jena disagreements reflect graph-node filtering; retained as explicit normative interpretation, not Jena passes |
| Original W3C values_and_path and zero_or_more_set_start | 2 | 2 | Candidate and Jena both match the published expected results |
| Adversarial result-comparison unit tests | 8 | — | All pass |

The original suite SHA-256 is `5c4dbfe3da5b3fe16082917647282fa9e34267db5261f323ad1a3e8df95e9143`, registered before candidate outputs. It covers a cyclic diamond, self-loop, inverse, alternatives, sequence multiplicity, closure reachability, negated sets, disconnected endpoints, empty graphs, literal term identity, repeated VALUES, OPTIONAL, EXISTS, UNION, ordered LIMIT/OFFSET and an unprojected blank-node intermediate. RDF graph set semantics are checked using a repeated source triple. SELECT comparison preserves complete RDF-term multisets and unbound states. One explicitly ordered case also requires the full result sequence. There is no floating-point tolerance or COUNT datatype relaxation.

This is a finite default-graph fixture suite, not all SPARQL path conformance, named-graph coverage, large-scale performance or embedded Python API validation. Projected blank-node identifier/isomorphism matching is outside scope. Ordinary strings and explicit xsd:string are compared as the same RDF term; typed numeric lexical forms remain distinct. No existing analytical benchmark gates or results were changed.

## Why the Jena disagreements remain visible

For the two original constant/constant ASK cases, both endpoints are the same absent IRI and the operator is p*. Jena returns false on both the nonempty and empty fixtures. The original frozen expected Boolean is true. The SPARQL ALP definition starts by adding the input term; the bound/bound star rule tests reachability in that result. We retain the true contract and the false oracle response separately. [SPARQL 1.1 property-path evaluation](https://www.w3.org/TR/sparql11-query/#defn_evalPP).

In the exploratory supplement, Jena returns a zero-length row for a variable supplied by VALUES/BIND even when that term is absent or occurs only as a predicate. TripleLight returns no row. The variable/variable definition enumerates graph subjects and objects. The official values_and_path fixture likewise expects no result for a query-provided term over an empty graph. This supports the candidate's graph-node rule, but those five exploratory cases are not exact Jena matches. The official control and a constant-endpoint empty-graph control both pass in both engines. [W3C property-path test descriptions](https://w3c.github.io/rdf-tests/sparql/sparql11/property-path/index.html).

Sequence and alternative controls preserve duplicate mappings; closure controls deduplicate reachable endpoints. Raw outputs and counts are included, so a passing total cannot hide a wrong multiplicity.

## Build and execution evidence

Source commit: `52fd1d715eff6d6d0bc851fccd0ea495cd6aa34f`, branch property-paths. A GitHub archive supplied 46 selected Rust/Cargo source files, all SHA-bound before and after compilation. The clean source snapshot built with `cargo build --locked --release --bin triplelight -j1`, using an existing agent-owned target cache. The copied native binary SHA-256 is `56a3569a10f0a07dcd5c5890d1a778a094e603ec89bdec0064a7426f24d26519`. No author source was patched. The binary and bulky source/cache trees are not copied into this report.

Each Jena reference runs arq over its own tiny fixture. Every candidate query runs the pinned CLI against that fixture, with a 45-second process timeout and one Rayon thread. This is output qualification, not timing. Source, suite, query, fixture, reference and binary identities recheck after execution; all recorded build/supervisor processes are absent. The owned machine lock was released after those checks. The scratch supervisor applies conservative sampled guards under the already approved allocation; it is not a hard memory or disk quota.

The generic oracle-anchor pass ends nonzero with 43/45 because of the two retained Jena disagreements. Before inspecting candidate outputs, oracle-adjudication.json documented their original true anchors and required separate Jena and contract scores. The candidate evaluation gives 43 Jena matches and 45 declared-contract passes. The generic supplement candidate gate ends nonzero with 2/7 Jena matches; its five differences were interpreted afterward using the formal rule and original W3C fixture. That retrospective interpretation is disclosed and does not rewrite its failed generic gate. An accidental second attempt to reuse an existing supplemental oracle directory was rejected before requests; its failed setup receipt is also preserved.

## Files and reproduction

suite.json, fixtures/, queries/ and qualify_paths.py form the reusable oracle/CLI harness; README.md describes its command-line interface. test_comparison.py checks duplicate counts, row/sequence ordering, projection, unbound states, lexical identity and ASK/SELECT separation. Original and supplementary attempts retain every raw JSON response, stderr, command and exit status. evaluate_candidate.py is the fixed-candidate diagnostic used for separate contract/Jena scoring; it deliberately references the recorded build and adjudication. w3c-controls/ includes source URLs/hashes, expected XML, parsed expectations and both-engine outcomes. Historical absolute paths require explicit adaptation.

SHA256SUMS.json hashes every published file except itself. No credentials, database blobs, executable binaries or private keys are included. These results support review of the tested property-path implementation only; they are not a blanket approval of every new function in the commit.
