use std::fmt;

/// Why a validation could not produce a report: the shapes graph is malformed, or the request needs something this
/// implementation does not define. A data graph that violates the shapes is not an error: it is a report.
#[derive(Debug)]
pub enum ValidationError {
    /// The shapes graph breaks a rule of SHACL Core (a shape without a valid `sh:path`, a malformed list, a bad parameter).
    MalformedShape(String),
    /// An `rdf:List` is malformed: a missing `rdf:first` or `rdf:rest`, a cycle, or a node with two values.
    MalformedList(String),
    /// SHACL leaves recursive shape references undefined; they are reported and not evaluated.
    UndefinedRecursion(String),
    /// Entailment or a feature this module does not provide (for example SHACL-SPARQL), or a limit it keeps: shape references nested more
    /// than 128 deep (`shapes::MAX_SHAPE_DEPTH`) and regular expressions the engine cannot compile.
    Unsupported(String),
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationError::MalformedShape(m) => write!(f, "malformed shape: {m}"),
            ValidationError::MalformedList(m) => write!(f, "malformed list: {m}"),
            ValidationError::UndefinedRecursion(m) => write!(f, "recursive shape reference (undefined in SHACL): {m}"),
            ValidationError::Unsupported(m) => write!(f, "unsupported: {m}"),
        }
    }
}

impl std::error::Error for ValidationError {}
