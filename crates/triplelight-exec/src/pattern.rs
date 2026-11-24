//! Compiled triple patterns: the vocabulary shared by the BGP executor (`bgp.rs`), range-filter pushdown (`range.rs`)
//! and the planners that choose between them. A pattern is `[Slot; 3]` (subject, predicate, object), each slot either
//! a dictionary constant or a row column.
use crate::model::TermId;
use std::collections::{HashMap, HashSet};

/// One position of a compiled triple pattern: a constant (`None` when the term is not in the dictionary, so the
/// pattern matches nothing) or the row column of a variable.
#[derive(Copy, Clone, Debug)]
pub(super) enum Slot { Const(Option<TermId>), Var(usize) }

/// Variables of one pattern, in slot order.
pub(super) fn vars_of(p: &[Slot; 3]) -> Vec<usize> { p.iter().filter_map(|s| if let Slot::Var(v) = s { Some(*v) } else { None }).collect() }

/// No variable occurs twice in `p`.
pub(super) fn no_repeated_var(p: &[Slot; 3]) -> bool {
    let vs = vars_of(p);
    vs.iter().collect::<HashSet<_>>().len() == vs.len()
}

/// GYO reduction: a BGP is acyclic iff repeatedly removing variables that occur in one pattern only, and patterns
/// whose variables are contained in another pattern's, empties the hypergraph.
pub(super) fn cyclic(pats: &[[Slot; 3]]) -> bool {
    let mut edges: Vec<HashSet<usize>> = pats.iter().map(|p| vars_of(p).into_iter().collect()).collect();
    loop {
        let mut changed = false;
        let mut occ: HashMap<usize, usize> = HashMap::new();
        for e in &edges { for v in e { *occ.entry(*v).or_default() += 1 } }
        for e in edges.iter_mut() {
            let before = e.len();
            e.retain(|v| occ[v] > 1);
            changed |= e.len() != before;
        }
        let n = edges.len();
        let mut keep = vec![true; n];
        for i in 0..n {
            if edges[i].is_empty() { keep[i] = false; continue }
            if (0..n).any(|j| j != i && keep[j] && edges[i].is_subset(&edges[j]) && (edges[i] != edges[j] || j < i)) { keep[i] = false }
        }
        let mut k = keep.iter();
        edges.retain(|_| *k.next().unwrap());
        changed |= edges.len() != n;
        if edges.is_empty() { return false }
        if !changed { return true }
    }
}
