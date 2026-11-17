//! rdfdb: single-machine RDF store. See DESIGN.md.
pub mod model;
pub mod storage;
pub mod opt;
pub mod exec;
pub mod store;

#[cfg(unix)]
pub mod embedded;
