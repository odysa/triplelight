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

/// One change of a write: insert or delete a triple.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change { Insert, Delete }

/// The graph a change applies to (task #36). Named graphs are given as IRIs, not IDs: a graph the dictionary has not
/// seen yet is interned in the same commit as its first quads.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum GraphTarget { Default, Named(oxrdf::NamedNode) }

/// The graphs a `DROP` or `CLEAR` applies to, as in SPARQL 1.1 Update.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum GraphScope {
    Graph(oxrdf::NamedNode),
    /// The default graph, which always exists: dropping it only empties it.
    Default,
    /// Every named graph.
    Named,
    /// The default graph and every named graph.
    All,
}

/// One step of a dataset update. A list of steps is applied in order and atomically (all or none), against the
/// state the previous steps left.
///
/// Existence rules, checked against that state: `Create` of a graph in the catalog fails unless `silent`;
/// `Drop`/`Clear` of `GraphScope::Graph` naming an absent graph fails unless `silent` (and then does nothing).
/// `Clear` keeps the graph in the catalog, `Drop` removes it. Inserting into an absent named graph adds it to the
/// catalog; deleting its last triple keeps it there.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DatasetChange {
    Quad(GraphTarget, Change, oxrdf::Triple),
    Create { graph: oxrdf::NamedNode, silent: bool },
    Drop { target: GraphScope, silent: bool },
    Clear { target: GraphScope, silent: bool },
}
