//! Plan contract between optimizer (task #3) and executor (task #4).
use crate::model::{Perm, TermId};

/// A position in a triple pattern: a bound term (already dictionary-encoded) or a variable index.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Slot { Const(TermId), Var(u32) }

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TriplePattern { pub s: Slot, pub p: Slot, pub o: Slot }

/// Physical join tree for one basic graph pattern. `est` = estimated output rows.
#[derive(Clone, Debug)]
pub enum JoinTree {
    /// Identity relation: one empty binding (empty basic graph pattern).
    Unit,
    /// Range scan of `perm` with the pattern's constants as the key prefix; output sorted by the next key slot.
    Scan { pattern: TriplePattern, perm: Perm, est: f64 },
    /// Per outer binding, probe the inner triple pattern using constants and bound variables.
    IndexNestedLoop { outer: Box<JoinTree>, pattern: TriplePattern, perm: Perm, est: f64 },
    /// Both inputs sorted on `var`.
    MergeJoin { var: u32, left: Box<JoinTree>, right: Box<JoinTree>, est: f64 },
    /// Build on `build`, probe with `probe`, keyed on `vars` (may be empty = cross product).
    HashJoin { vars: Vec<u32>, build: Box<JoinTree>, probe: Box<JoinTree>, est: f64 },
    /// Worst-case-optimal join over `vars` in this order; each child is a Scan whose perm puts its join vars in that order.
    Leapfrog { vars: Vec<u32>, children: Vec<JoinTree>, est: f64 },
    /// Pattern can never match (a constant is absent from the dictionary).
    Empty,
}

impl JoinTree {
    pub fn est(&self) -> f64 {
        match self {
            JoinTree::Scan { est, .. } | JoinTree::MergeJoin { est, .. }
            | JoinTree::HashJoin { est, .. } | JoinTree::Leapfrog { est, .. }
            | JoinTree::IndexNestedLoop { est, .. } => *est,
            JoinTree::Empty => 0.0,
            JoinTree::Unit => 1.0,
        }
    }
}
