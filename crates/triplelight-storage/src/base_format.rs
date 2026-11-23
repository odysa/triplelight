//! Definitions of the on-disk base format that its writers (`external_build`, `lazy_dictionary`, `term_block_codec`)
//! and its reader (`disk_snapshot`) share: the limits a stored term must respect, the order sets a base may keep, and
//! the names of its files. A layer below all four, so none of them depends on another for the format's facts (#107).

/// The longest N-Triples term text a base can hold (task #45: the WAL refuses longer terms at commit).
pub(crate) const MAX_TERM: usize = triplelight_core::ntriples::MAX_TOKEN;
/// Which triple (and quad) orders a base keeps, as bits of their `Perm::ALL` positions (task #73).
pub const ALL_ORDERS: u8 = 0b11_1111;
/// SPO, PSO, POS and OSP: kept by every base. SOP and OPS are served from them when absent (`derived_order`).
pub const REQUIRED_ORDERS: u8 = 0b01_1101;
/// The RDF 1.2 triple-term parts table of an `RDFSNAP7` base.
pub(crate) const TRIPLE_TERMS_FILE: &str = "triple_terms";
