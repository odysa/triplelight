//! The error shared by every reader of an RDF text format (N-Triples, N-Quads, Turtle, TriG): the lowest layer of this crate's
//! parsing code, which `ntriples`, `turtle` and the `rdf` facade all depend on and which depends on none of them.

/// A failure reading an RDF document: the reader's own error, or invalid syntax.
#[derive(Debug)]
pub enum ReadError { Io(std::io::Error), Syntax(String) }

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { ReadError::Io(e) => e.fmt(f), ReadError::Syntax(m) => f.write_str(m) }
    }
}
impl std::error::Error for ReadError {}
impl From<ReadError> for std::io::Error {
    fn from(e: ReadError) -> Self {
        match e { ReadError::Io(e) => e, ReadError::Syntax(m) => std::io::Error::new(std::io::ErrorKind::InvalidData, m) }
    }
}
