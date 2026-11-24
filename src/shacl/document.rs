//! The entry point the CLI and the bindings share: validate a data document against a shapes document, both RDF text.
//! It parses the two with the existing readers and calls [`validate`], keeping the three ways it can fail apart: a
//! syntax error in either document, a shapes graph SHACL rejects, or a feature this module does not provide. A data
//! graph that violates the shapes is none of these: it is a report with `conforms` false.
use super::{validate, ValidationError, ValidationReport};
use crate::rdf::{parse_triples, Format};
use std::fmt;

/// Which of the two documents a syntax error is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Document { Data, Shapes }

#[derive(Debug)]
pub enum DocumentError {
    /// One of the documents does not parse.
    Syntax { document: Document, message: String },
    /// The shapes parse but SHACL Core rejects them, or they need something this module does not define.
    Validation(ValidationError),
}

impl fmt::Display for DocumentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DocumentError::Syntax { document: Document::Data, message } => write!(f, "syntax error in the data graph: {message}"),
            DocumentError::Syntax { document: Document::Shapes, message } => write!(f, "syntax error in the shapes graph: {message}"),
            DocumentError::Validation(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DocumentError {}

/// Validates `data` against `shapes`, each RDF text in its own format (N-Triples or Turtle).
pub fn validate_documents(data: &[u8], data_format: Format, shapes: &[u8], shapes_format: Format) -> Result<ValidationReport, DocumentError> {
    let parse = |text: &[u8], format, document| parse_triples(text, format).map_err(|message| DocumentError::Syntax { document, message });
    let data = parse(data, data_format, Document::Data)?;
    let shapes = parse(shapes, shapes_format, Document::Shapes)?;
    validate(&data, &shapes).map_err(DocumentError::Validation)
}
