//! Structural conversion of oxrdf terms (what the oxttl comparator yields) into TripleLight's, so the differential
//! tests compare statements with `==` on one type (task #45, step 4).
#![allow(dead_code)]
use triplelight_rdf as tl;

pub fn named(n: oxrdf::NamedNode) -> tl::NamedNode { tl::NamedNode::new_unchecked(n.into_string()) }
pub fn blank(b: oxrdf::BlankNode) -> tl::BlankNode { tl::BlankNode::new_unchecked(b.into_string()) }
pub fn subject(s: oxrdf::NamedOrBlankNode) -> tl::NamedOrBlankNode {
    match s { oxrdf::NamedOrBlankNode::NamedNode(n) => named(n).into(), oxrdf::NamedOrBlankNode::BlankNode(b) => blank(b).into() }
}
pub fn term(t: oxrdf::Term) -> tl::Term {
    match t {
        oxrdf::Term::NamedNode(n) => named(n).into(),
        oxrdf::Term::BlankNode(b) => blank(b).into(),
        oxrdf::Term::Literal(l) => {
            let (value, datatype, language) = l.destruct();
            match (datatype, language) {
                (_, Some(lang)) => tl::Literal::new_language_tagged_literal_unchecked(value, lang),
                (Some(dt), None) => tl::Literal::new_typed_literal(value, named(dt)),
                (None, None) => tl::Literal::new_simple_literal(value),
            }
            .into()
        }
    }
}
pub fn graph(g: oxrdf::GraphName) -> tl::GraphName {
    match g {
        oxrdf::GraphName::NamedNode(n) => named(n).into(),
        oxrdf::GraphName::BlankNode(b) => blank(b).into(),
        oxrdf::GraphName::DefaultGraph => tl::GraphName::DefaultGraph,
    }
}
pub fn triple(t: oxrdf::Triple) -> tl::Triple { tl::Triple::new(subject(t.subject), named(t.predicate), term(t.object)) }
pub fn quad(q: oxrdf::Quad) -> tl::Quad { tl::Quad::new(subject(q.subject), named(q.predicate), term(q.object), graph(q.graph_name)) }
