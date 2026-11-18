//! Shared types between storage, optimizer and executor.

/// Dictionary-encoded term. The top 4 bits tag the kind; inline kinds carry their value in the low 60 bits.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TermId(pub u64);

/// The six orderings of (subject, predicate, object).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Perm { Spo, Sop, Pso, Pos, Osp, Ops }

impl Perm {
    pub const ALL: [Perm; 6] = [Perm::Spo, Perm::Sop, Perm::Pso, Perm::Pos, Perm::Osp, Perm::Ops];
    /// Positions of S=0, P=1, O=2 in this ordering's key.
    pub fn order(self) -> [usize; 3] {
        match self {
            Perm::Spo => [0, 1, 2], Perm::Sop => [0, 2, 1], Perm::Pso => [1, 0, 2],
            Perm::Pos => [1, 2, 0], Perm::Osp => [2, 0, 1], Perm::Ops => [2, 1, 0],
        }
    }
}

/// Columnar batch of triples in a permutation's key order (col[0] = first key component).
#[derive(Default, Debug, Clone)]
pub struct TripleBatch { pub cols: [Vec<TermId>; 3] }

impl TripleBatch {
    pub fn len(&self) -> usize { self.cols[0].len() }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}
