//! RDF 1.1 term types (task #45, step 4): named nodes, blank nodes, literals, variables, triples, quads and graph
//! names, with the API, equality, ordering, hashing and `Display` text of `oxrdf` 0.3.5 (RDF 1.2 triple terms and
//! directional literals only with the `rdf-12` feature, as in oxrdf; without `Dataset`, `Graph` and serde), so
//! stores and outputs written with oxrdf terms stay identical. IRIs are checked by `triplelight-iri` and language tags
//! by [`langtag`].
//! Derived from oxrdf (MIT); see THIRD_PARTY_NOTICES.md.

mod blank_node;
#[doc(hidden)]
pub mod langtag;
mod literal;
mod named_node;
mod syntax;
mod triple;
mod variable;
pub mod vocab;

pub use crate::blank_node::{BlankNode, BlankNodeRef};
pub use crate::langtag::LanguageTagParseError;
#[cfg(feature = "rdf-12")]
pub use crate::literal::BaseDirection;
#[cfg(feature = "rdf-12")]
pub use crate::triple::{triple_term_depth, MAX_TRIPLE_TERM_DEPTH};
pub use crate::literal::{Literal, LiteralRef};
pub use crate::named_node::{IriParseError, NamedNode, NamedNodeRef};
pub use crate::syntax::{BlankNodeIdParseError, TermParseError, VariableNameParseError};
pub use crate::triple::{
    GraphName, GraphNameRef, NamedOrBlankNode, NamedOrBlankNodeRef, Quad, QuadRef, Term, TermRef, Triple, TripleRef,
    TryFromTermError,
};
pub use crate::variable::{Variable, VariableRef};
