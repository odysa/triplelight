//! SPARQL evaluation through the embedded API: operators and functions, property paths, the optimizer and its switches
//! (range and count pushdown), result formats, and SPARQL 1.2. One test target, so the library builds once for all of them.
//! `plan_trace.rs` stays its own target: it needs a process in which `TRIPLELIGHT_TRACE_PLAN` is on before any query runs.
mod count_pushdown;
mod exec;
mod functions;
mod optimizer;
mod paths;
mod range_pushdown;
mod results;
#[cfg(feature = "rdf-12")]
mod sparql12;
