//! The W3C RDF 1.2 syntax and evaluation suites for the four line/terse formats.
use oxrdf::{
    CanonicalizationAlgorithm, Dataset, Graph, NamedNodeRef, NamedOrBlankNodeRef, TermRef,
};
use std::fs;
use std::path::Path;
use std::process::Command;
use triplelight_core::ntriples::{read_nquads, read_ntriples};
use triplelight_core::turtle::TurtleReader;
use triplelight_rdf as tl;

const COMMIT: &str = "56076c0efec4c353efcae2971be8f6005294ac64";
const ORIGIN: &str = "https://w3c.github.io/rdf-tests/";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const MF: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#";
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

fn named(node: tl::NamedNode) -> oxrdf::NamedNode {
    oxrdf::NamedNode::new_unchecked(node.into_string())
}

fn blank(node: tl::BlankNode) -> oxrdf::BlankNode {
    oxrdf::BlankNode::new_unchecked(node.into_string())
}

fn subject(node: tl::NamedOrBlankNode) -> oxrdf::NamedOrBlankNode {
    match node {
        tl::NamedOrBlankNode::NamedNode(node) => named(node).into(),
        tl::NamedOrBlankNode::BlankNode(node) => blank(node).into(),
    }
}

fn term(value: tl::Term) -> oxrdf::Term {
    match value {
        tl::Term::NamedNode(node) => named(node).into(),
        tl::Term::BlankNode(node) => blank(node).into(),
        tl::Term::Triple(node) => {
            let node = *node;
            oxrdf::Triple::new(
                subject(node.subject),
                named(node.predicate),
                term(node.object),
            )
            .into()
        }
        tl::Term::Literal(value) => {
            let (value, datatype, language, direction) = value.destruct();
            match (datatype, language, direction) {
                (_, Some(language), Some(direction)) => {
                    let direction = match direction {
                        tl::BaseDirection::Ltr => oxrdf::BaseDirection::Ltr,
                        tl::BaseDirection::Rtl => oxrdf::BaseDirection::Rtl,
                    };
                    oxrdf::Literal::new_directional_language_tagged_literal_unchecked(
                        value, language, direction,
                    )
                }
                (_, Some(language), None) => {
                    oxrdf::Literal::new_language_tagged_literal_unchecked(value, language)
                }
                (Some(datatype), None, None) => {
                    oxrdf::Literal::new_typed_literal(value, named(datatype))
                }
                (None, None, None) => oxrdf::Literal::new_simple_literal(value),
                (_, None, Some(_)) => unreachable!("direction requires a language"),
            }
            .into()
        }
    }
}

fn quad(value: tl::Quad) -> oxrdf::Quad {
    let graph = match value.graph_name {
        tl::GraphName::NamedNode(node) => oxrdf::GraphName::NamedNode(named(node)),
        tl::GraphName::BlankNode(node) => oxrdf::GraphName::BlankNode(blank(node)),
        tl::GraphName::DefaultGraph => oxrdf::GraphName::DefaultGraph,
    };
    oxrdf::Quad::new(
        subject(value.subject),
        named(value.predicate),
        term(value.object),
        graph,
    )
}

fn parse(data: &[u8], format: &str, base: &str) -> Result<Dataset, tl::ReadError> {
    match format {
        "NTriples" => read_ntriples(data)
            .map(|result| result.map(|value| quad(value.in_graph(tl::GraphName::DefaultGraph))))
            .collect(),
        "NQuads" => read_nquads(data).map(|result| result.map(quad)).collect(),
        "Turtle" | "Trig" => TurtleReader::with_base(data, format == "Trig", base)?
            .map(|result| result.map(quad))
            .collect(),
        _ => unreachable!("unknown format {format}"),
    }
}

fn isomorphic(mut expected: Dataset, mut actual: Dataset) -> bool {
    expected.canonicalize(CanonicalizationAlgorithm::Unstable);
    actual.canonicalize(CanonicalizationAlgorithm::Unstable);
    expected == actual
}

fn fixture(root: &Path, iri: &str) -> Vec<u8> {
    let relative = iri.strip_prefix(ORIGIN).expect("W3C fixture IRI");
    fs::read(root.join(relative)).unwrap_or_else(|error| panic!("{iri}: {error}"))
}

fn property<'a>(
    graph: &'a Graph,
    subject: NamedOrBlankNodeRef<'a>,
    prefix: &str,
    local: &str,
) -> TermRef<'a> {
    let predicate = format!("{prefix}{local}");
    let mut matches =
        graph.objects_for_subject_predicate(subject, NamedNodeRef::new_unchecked(&predicate));
    let value = matches
        .next()
        .unwrap_or_else(|| panic!("missing {local} on {subject}"));
    assert!(
        matches.next().is_none(),
        "multiple {local} values on {subject}"
    );
    value
}

fn node(term: TermRef<'_>) -> NamedOrBlankNodeRef<'_> {
    match term {
        TermRef::NamedNode(value) => value.into(),
        TermRef::BlankNode(value) => value.into(),
        _ => panic!("expected IRI or blank node: {term}"),
    }
}

fn iri(term: TermRef<'_>) -> &str {
    match term {
        TermRef::NamedNode(value) => value.as_str(),
        _ => panic!("expected an IRI: {term}"),
    }
}

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
    let root = std::env::var_os("TL_RDF_TESTS").expect("set TL_RDF_TESTS to the pinned checkout");
    let root = Path::new(&root);
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(head.status.success());
    assert_eq!(String::from_utf8(head.stdout).unwrap().trim(), COMMIT);

    let mut failures = Vec::new();
    let mut total = 0;
    for &(suite, format, count) in SUITES {
        let manifest = format!("{ORIGIN}rdf/rdf12/{suite}/manifest.ttl");
        let contents = fixture(root, &manifest);
        let graph: Graph = oxttl::TurtleParser::new()
            .with_base_iri(&manifest)
            .unwrap()
            .for_slice(&contents)
            .collect::<Result<_, _>>()
            .unwrap();
        let entries = NamedNodeRef::new_unchecked(
            "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#entries",
        );
        let mut heads = graph
            .triples_for_predicate(entries)
            .map(|triple| triple.object);
        let mut next = heads.next().expect("manifest entry list");
        assert!(heads.next().is_none());
        let mut visited = std::collections::HashSet::new();
        while next != TermRef::NamedNode(oxrdf::vocab::rdf::NIL) {
            let entry = node(property(&graph, node(next), RDF, "first"));
            next = property(&graph, node(next), RDF, "rest");
            assert!(visited.insert(entry), "repeated case {entry}");
            let data = iri(property(&graph, entry, MF, "action"));
            let kind = iri(property(&graph, entry, RDF, "type"));
            let outcome = parse(&fixture(root, data), format, data);
            let suffix = kind
                .strip_prefix(RDFT)
                .and_then(|kind| kind.strip_prefix("Test"))
                .and_then(|kind| kind.strip_prefix(format))
                .expect("unexpected test kind");
            let failure = match suffix {
                "PositiveSyntax" => outcome.err().map(|error| error.to_string()),
                "NegativeSyntax" => match outcome {
                    Err(tl::ReadError::Syntax(_)) => None,
                    Err(error) => Some(error.to_string()),
                    Ok(_) => Some("negative test accepted".into()),
                },
                "Eval" => {
                    let expected =
                        expected_output(root, iri(property(&graph, entry, MF, "result")));
                    match outcome {
                        Ok(actual) => {
                            (!isomorphic(expected, actual)).then(|| "dataset differs".into())
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
        assert_eq!(visited.len(), count, "{suite}");
        total += count;
    }
    assert_eq!(total, 238);
    assert!(
        failures.is_empty(),
        "{}/{} failed: {failures:?}",
        failures.len(),
        total
    );
}
