//! TripleLight's own SPARQL 1.1 front end: tokenizer, parser and algebra (W3C SPARQL 1.1 Query Language §18–19,
//! SPARQL 1.1 Update §3–4). Written from the specification.
pub mod algebra;
mod lexer;
mod parser;

pub use algebra::*;
pub use parser::{in_scope, parse_query, parse_update, ParseError};
