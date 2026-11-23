//! The pinned W3C SPARQL 1.2 query and update tests.
#[path = "w3c/eval.rs"]
mod eval;
#[path = "w3c/terms.rs"]
mod terms;
mod w3c;

use triplelight_sparql::{parse_query, parse_update};
use w3c::{MF, RDF, entries, fixture, iri, load_manifest, pinned_checkout, property};

// The test revision predates the November 19 SPARQL 1.2 Query and Update drafts.
const SUITES: &[(&str, usize)] = &[
    ("codepoint-escapes", 14),
    ("eval-triple-terms", 41),
    ("expression", 5),
    ("grouping", 2),
    ("lang-basedir", 13),
    ("rdf11", 3),
    ("syntax-triple-terms-negative", 65),
    ("syntax-triple-terms-positive", 113),
    ("syntax", 6),
    ("version", 9),
];

fn assert_floor(suite: &str, passed: usize, failures: &[String], total: usize) {
    println!("{suite}: {passed}/{total} W3C cases pass");
    assert_eq!(passed, total, "{suite} fell below its floor: {failures:?}");
    assert!(failures.is_empty(), "{suite} has failing cases: {failures:?}");
}

#[test]
#[ignore = "requires the exact W3C rdf-tests checkout in TL_RDF_TESTS"]
fn sparql12_w3c_query_and_update_syntax() {
    let root = pinned_checkout();
    let mut totals = std::collections::BTreeMap::new();
    let mut failures = Vec::new();
    let mut passed = 0;
    for &(suite, count) in SUITES {
        let graph = load_manifest(&root, &format!("sparql/sparql12/{suite}/manifest.ttl"));
        let cases = entries(&graph);
        assert_eq!(cases.len(), count, "{suite}");
        for entry in cases {
            let kind = iri(property(&graph, entry, RDF, "type"))
                .strip_prefix(MF)
                .expect("SPARQL test type");
            *totals.entry(kind.to_owned()).or_insert(0) += 1;
            let (update, positive) = match kind {
                "PositiveSyntaxTest" => (false, true),
                "NegativeSyntaxTest" => (false, false),
                "PositiveUpdateSyntaxTest" => (true, true),
                "NegativeUpdateSyntaxTest" => (true, false),
                "QueryEvaluationTest" | "UpdateEvaluationTest" => continue,
                _ => panic!("unknown test type {kind}"),
            };
            let action = iri(property(&graph, entry, MF, "action"));
            let text =
                String::from_utf8(fixture(&root, action)).expect("SPARQL source must be UTF-8");
            let status = if update {
                parse_update(&text, Some(action)).map(drop)
            } else {
                parse_query(&text, Some(action)).map(drop)
            };

            match (positive, status) {
                (true, Ok(())) | (false, Err(_)) => {
                    passed += 1;
                    println!("PASS {entry}");
                }
                (true, Err(error)) => {
                    eprintln!("FAIL {entry} {error}");
                    failures.push(entry.to_string());
                }
                (false, Ok(())) => {
                    eprintln!("FAIL {entry} negative syntax accepted");
                    failures.push(entry.to_string());
                }
            }
        }
    }
    for (kind, expected) in [
        ("PositiveSyntaxTest", 102),
        ("NegativeSyntaxTest", 81),
        ("PositiveUpdateSyntaxTest", 18),
        ("NegativeUpdateSyntaxTest", 2),
        ("QueryEvaluationTest", 65),
        ("UpdateEvaluationTest", 3),
    ] {
        assert_eq!(totals[kind], expected, "{kind}");
    }
    assert_floor("SPARQL 1.2 syntax", passed, &failures, 203);
}

#[test]
#[ignore = "requires the exact W3C rdf-tests checkout in TL_RDF_TESTS"]
fn sparql12_w3c_query_and_update_evaluation() {
    let root = pinned_checkout();
    let mut queries = 0;
    let mut updates = 0;
    let mut failures = Vec::new();
    let mut passed = 0;
    for &(suite, count) in SUITES {
        let graph = load_manifest(&root, &format!("sparql/sparql12/{suite}/manifest.ttl"));
        let cases = entries(&graph);
        assert_eq!(cases.len(), count, "{suite}");
        for entry in cases {
            let kind = iri(property(&graph, entry, RDF, "type"))
                .strip_prefix(MF)
                .unwrap();
            let update = match kind {
                "QueryEvaluationTest" => {
                    queries += 1;
                    false
                }
                "UpdateEvaluationTest" => {
                    updates += 1;
                    true
                }
                _ => continue,
            };
            match eval::evaluate(&root, &graph, entry, update) {
                Ok(()) => {
                    passed += 1;
                    println!("PASS {entry}");
                }
                Err(error) => {
                    eprintln!("FAIL {entry} {error}");
                    failures.push(entry.to_string());
                }
            }
        }
    }
    assert_eq!((queries, updates), (65, 3));
    assert_floor("SPARQL 1.2 evaluation", passed, &failures, 68);
}
