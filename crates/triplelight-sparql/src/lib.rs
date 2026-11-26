//! TripleLight's own SPARQL 1.1 front end: tokenizer, parser and algebra (W3C SPARQL 1.1 Query Language §18–19,
//! SPARQL 1.1 Update §3–4). Written from the specification. The `sparql-12` feature adds the SPARQL 1.2 query syntax
//! (W3C Working Draft 2026-11-19): triple terms, reified triples, annotations, base directions and `VERSION`.
pub(crate) use triplelight_core::rdf;

pub mod algebra;
mod lexer;
mod parser;

pub use algebra::*;
pub use lexer::statement_end;
pub use parser::{in_scope, is_internal_variable, parse_query, parse_query_with_aggregates, parse_update, ParseError, INTERNAL_PREFIX};
