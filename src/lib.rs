//! TripleLight: an embedded analytical RDF database. See DESIGN.md.
pub mod model;
pub mod rdf;
pub mod storage;
pub mod opt;
pub mod exec;
pub mod store;

#[cfg(unix)]
pub mod embedded;
