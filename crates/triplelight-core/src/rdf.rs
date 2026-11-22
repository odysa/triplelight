//! RDF terms and text formats as used by TripleLight's public API. Bindings (Python, Node, WASM) import these from
//! here instead of depending on the underlying parsing and term crates, so those can be replaced without changing
//! any binding.
pub use triplelight_rdf::{BlankNode, GraphName, Literal, NamedNode, NamedOrBlankNode, Quad, Term, TermRef, Triple, Variable};

/// RDF dataset text formats TripleLight reads (task #36).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuadFormat { NQuads, TriG }

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

/// Every quad of an N-Quads or TriG document, in document order.
pub fn read_quads(reader: impl std::io::Read, format: QuadFormat) -> Result<Vec<Quad>, ReadError> {
    match format {
        QuadFormat::NQuads => crate::ntriples::read_nquads(std::io::BufReader::new(reader)).collect(),
        QuadFormat::TriG => crate::turtle::read_trig(std::io::BufReader::new(reader)).collect(),
    }
}

/// RDF text formats TripleLight reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format { NTriples, Turtle }

impl Format {
    /// `"ntriples"`/`"nt"` or `"turtle"`/`"ttl"`.
    pub fn from_name(name: &str) -> Option<Format> {
        match name { "ntriples" | "nt" => Some(Format::NTriples), "turtle" | "ttl" => Some(Format::Turtle), _ => None }
    }
}

/// Parses a whole document; on the first syntax error returns its message and no triples.
pub fn parse_triples(data: &[u8], format: Format) -> Result<Vec<Triple>, String> {
    match format {
        Format::NTriples => crate::ntriples::read_ntriples_slice(data).collect::<Result<_, _>>().map_err(|e| e.to_string()),
        Format::Turtle => crate::turtle::read_turtle_slice(data).collect::<Result<_, _>>().map_err(|e| e.to_string()),
    }
}
