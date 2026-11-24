//! SHACL Core (W3C Recommendation, 2017-07-20): validation of an RDF data graph against a shapes graph, both given as
//! triples. The module uses only the platform-independent RDF types of [`crate::rdf`]; the CLI and the bindings load the
//! two graphs with the existing RDF readers and call [`validate`].
//!
//! Layers, top to bottom: `evaluator` (targets, value nodes, the recursion guard, results) uses `shapes` (parsing) and
//! `constraints` (the constraint components), which use `path` and `graph`; `report` holds the result types, and `error`
//! and `vocab` are leaves. SHACL-SPARQL and the SHACL 1.2 drafts are outside this module.
mod constraints;
mod document;
mod error;
mod evaluator;
mod graph;
mod path;
mod report;
mod shapes;
mod vocab;

pub use constraints::{CheckNode, Constraint, ConstraintContext};
pub use document::{validate_documents, Document, DocumentError};
pub use error::ValidationError;
pub use evaluator::{validate, Evaluator};
pub use graph::Graph;
pub use path::Path;
pub use report::{ConstraintViolation, Outcome, Severity, ValidationReport, ValidationResult};
pub use shapes::{Shape, ShapesGraph, Target};

#[cfg(test)]
mod tests;
