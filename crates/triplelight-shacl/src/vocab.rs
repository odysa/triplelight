//! The IRIs SHACL Core and its graph adapter use.
use crate::rdf::NamedNode;

const SH: &str = "http://www.w3.org/ns/shacl#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";

/// `sh:<local>`.
pub fn sh(local: &str) -> NamedNode { NamedNode::new_unchecked(format!("{SH}{local}")) }
/// `rdf:<local>`.
pub fn rdf(local: &str) -> NamedNode { NamedNode::new_unchecked(format!("{RDF}{local}")) }
/// `rdfs:<local>`.
pub fn rdfs(local: &str) -> NamedNode { NamedNode::new_unchecked(format!("{RDFS}{local}")) }
