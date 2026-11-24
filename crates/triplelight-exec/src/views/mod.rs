//! The graph views a query reads besides the store itself: dataset views (the empty default graph, the FROM merge)
//! and the RDFS-entailed views over any of them.
pub(crate) mod dataset;
pub(crate) mod entail;
