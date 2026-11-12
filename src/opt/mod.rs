//! Optimizer (owner: Jianlin-Su, task #3).
pub mod plan;
pub mod stats;
pub use stats::{Estimate, Stats, StatsError, q_error};
pub mod planner;
pub use planner::{PlanDecision, PlannerOptions, plan_bgp, plan_bgp_with_options};
