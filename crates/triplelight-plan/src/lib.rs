//! Query planning: statistics (characteristic sets, histograms) and cost-based join ordering.
pub(crate) use triplelight_core::{model, storage};

pub mod plan;
pub mod stats;
pub use stats::{Estimate, PredicatePairs, Stats, StatsError, q_error};
pub mod planner;
pub use planner::{BinaryJoinMode, PlanDecision, PlannerOptions, plan_bgp, plan_bgp_with_options};
