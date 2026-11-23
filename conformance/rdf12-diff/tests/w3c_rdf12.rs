//! The W3C RDF 1.2 syntax and evaluation suites for the four line/terse formats.
use oxrdf::Dataset;
use std::path::Path;
use triplelight_core::rdf::ReadError;

#[path = "w3c/terms.rs"]
mod terms;
mod w3c;
use terms::{isomorphic, parse_native, quad};
use w3c::{MF, RDF, entries, fixture, iri, load_manifest, pinned_checkout, property};

const RDFT: &str = "http://www.w3.org/ns/rdftest#";

// Suite pin above is the November 18, 2026 test revision. The draft dates are
// N-Triples Nov 22, N-Quads Nov 20, Turtle Nov 11 and TriG Nov 13.
const SUITES: &[(&str, &str, usize)] = &[
    ("rdf-n-triples/syntax", "NTriples", 34),
    ("rdf-n-quads/syntax", "NQuads", 31),
    ("rdf-turtle/syntax", "Turtle", 77),
    ("rdf-turtle/eval", "Turtle", 32),
    ("rdf-trig/syntax", "Trig", 38),
    ("rdf-trig/eval", "Trig", 26),
];

fn expected_output(root: &Path, path: &str) -> Dataset {
    let data = fixture(root, path);
    if path.ends_with(".nt") {
        oxttl::NTriplesParser::new()
            .for_slice(&data)
            .map(|value| value.map(|value| value.in_graph(oxrdf::GraphName::DefaultGraph)))
            .collect::<Result<_, _>>()
            .unwrap_or_else(|error| panic!("invalid reference {path}: {error}"))
    } else {
        assert!(path.ends_with(".nq"), "unknown reference format {path}");
        oxttl::NQuadsParser::new()
            .for_slice(&data)
            .collect::<Result<_, _>>()
            .unwrap_or_else(|error| panic!("invalid reference {path}: {error}"))
    }
}

#[test]
fn comparison_checks_nested_blank_node_identity_and_direction() {
    let parse = |data: &str| {
        oxttl::NQuadsParser::new()
            .for_slice(data.as_bytes())
            .collect::<Result<Dataset, _>>()
            .unwrap()
    };
    let expected = parse(concat!(
        "_:x <urn:p> <<( _:x <urn:p> \"text\"@en--ltr )>> <urn:g> .\n",
        "_:x <urn:q> _:y ."
    ));
    let renamed = concat!(
        "_:z <urn:p> <<( _:z <urn:p> \"text\"@en--ltr )>> <urn:g> .\n",
        "_:z <urn:q> _:w ."
    );
    assert!(isomorphic(expected.clone(), parse(renamed)));
    for wrong in [
        renamed.replace("<<( _:z", "<<( _:w"),
        renamed.replace("--ltr", "--rtl"),
        renamed.replace(" <urn:g>", ""),
    ] {
        assert!(!isomorphic(expected.clone(), parse(&wrong)), "{wrong}");
    }
}

#[test]
#[ignore = "requires the exact W3C rdf-tests checkout in TL_RDF_TESTS"]
fn rdf12_w3c_syntax_and_evaluation() {
    let root = pinned_checkout();
    let mut failures = Vec::new();
    let mut total = 0;
    for &(suite, format, count) in SUITES {
        let graph = load_manifest(&root, &format!("rdf/rdf12/{suite}/manifest.ttl"));
        let cases = entries(&graph);
        assert_eq!(cases.len(), count, "{suite}");
        for entry in cases {
            let data = iri(property(&graph, entry, MF, "action"));
            let kind = iri(property(&graph, entry, RDF, "type"));
            let outcome = parse_native(&fixture(&root, data), format, data);
            let suffix = kind
                .strip_prefix(RDFT)
                .and_then(|kind| kind.strip_prefix("Test"))
                .and_then(|kind| kind.strip_prefix(format))
                .expect("unexpected test kind");
            let failure = match suffix {
                "PositiveSyntax" => outcome.err().map(|error| error.to_string()),
                "NegativeSyntax" => match outcome {
                    Err(ReadError::Syntax(_)) => None,
                    Err(error) => Some(error.to_string()),
                    Ok(_) => Some("negative test accepted".into()),
                },
                "Eval" => {
                    let expected =
                        expected_output(&root, iri(property(&graph, entry, MF, "result")));
                    match outcome {
                        Ok(actual) => {
                            (!isomorphic(expected, actual.into_iter().map(quad).collect()))
                                .then(|| "dataset differs".into())
                        }
                        Err(error) => Some(error.to_string()),
                    }
                }
                _ => panic!("unknown test kind {kind}"),
            };
            if let Some(failure) = failure {
                eprintln!("FAIL {entry} {failure}");
                failures.push(entry.to_string());
            } else {
                println!("PASS {entry}");
            }
        }
        total += count;
    }
    assert_eq!(total, 238);
    println!(
        "RDF 1.2 parser: {}/238 W3C cases pass",
        total - failures.len()
    );
    assert!(
        failures.is_empty(),
        "{}/{} failed: {failures:?}",
        failures.len(),
        total
    );
}
