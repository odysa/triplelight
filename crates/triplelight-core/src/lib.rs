//! Types and traits shared by TripleLight's crates: term IDs and permutations, RDF terms, and the storage traits
//! the planner and executor are written against. Storage implementations live in `triplelight-storage`.
pub mod langtag;
pub mod model;
pub mod ntriples;
pub mod rdf;
pub mod turtle;
pub mod storage;
