use oxrdf::Dataset;
use oxrdf::dataset::CanonicalizationAlgorithm;
use triplelight_core::ntriples::{read_nquads, read_ntriples};
use triplelight_core::rdf::ReadError;
use triplelight_core::turtle::TurtleReader;
use triplelight_rdf as tl;

pub fn named(node: tl::NamedNode) -> oxrdf::NamedNode {
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

pub fn term(value: tl::Term) -> oxrdf::Term {
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

pub fn quad(value: tl::Quad) -> oxrdf::Quad {
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

pub fn parse_native(data: &[u8], format: &str, base: &str) -> Result<Vec<tl::Quad>, ReadError> {
    match format {
        "NTriples" => read_ntriples(data)
            .map(|result| result.map(|value| value.in_graph(tl::GraphName::DefaultGraph)))
            .collect(),
        "NQuads" => read_nquads(data).collect(),
        "Turtle" | "Trig" => TurtleReader::with_base(data, format == "Trig", base)?.collect(),
        _ => unreachable!("unknown format {format}"),
    }
}

pub fn isomorphic(mut expected: Dataset, mut actual: Dataset) -> bool {
    expected.canonicalize(CanonicalizationAlgorithm::Unstable);
    actual.canonicalize(CanonicalizationAlgorithm::Unstable);
    expected == actual
}
