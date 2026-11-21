//! RDF terms and text formats as used by TripleLight's public API. Bindings (Python, Node, WASM) import these from
//! here instead of depending on the underlying parsing and term crates, so those can be replaced without changing
//! any binding.
pub use oxrdf::{BlankNode, GraphName, Literal, NamedNode, NamedOrBlankNode, Quad, Term, TermRef, Triple, Variable};

/// RDF dataset text formats TripleLight reads (task #36).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuadFormat { NQuads, TriG }

/// A failure reading an RDF document: the reader's own error, or invalid syntax.
#[derive(Debug)]
pub enum ReadError { Io(std::io::Error), Syntax(String) }

/// Every quad of an N-Quads or TriG document, in document order.
pub fn read_quads(reader: impl std::io::Read, format: QuadFormat) -> Result<Vec<Quad>, ReadError> {
    let quads = match format {
        QuadFormat::NQuads => oxttl::NQuadsParser::new().for_reader(reader).collect::<Result<Vec<_>, _>>(),
        QuadFormat::TriG => oxttl::TriGParser::new().for_reader(reader).collect::<Result<Vec<_>, _>>(),
    };
    quads.map_err(|e| match e {
        oxttl::TurtleParseError::Io(e) => ReadError::Io(e),
        oxttl::TurtleParseError::Syntax(e) => ReadError::Syntax(e.to_string()),
    })
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
        Format::NTriples => oxttl::NTriplesParser::new().for_slice(data).collect::<Result<_, _>>().map_err(|e| e.to_string()),
        Format::Turtle => oxttl::TurtleParser::new().for_slice(data).collect::<Result<_, _>>().map_err(|e| e.to_string()),
    }
}
