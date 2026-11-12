//! Left-deep, order-aware subset DP for small BGPs, connected greedy extension for large BGPs.
use super::plan::{JoinTree, Slot, TriplePattern};
use super::stats::{Stats, cardinal, has_repeated_variable, scan_permutation, slots, variables};
use crate::model::Perm;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug)]
pub struct PlannerOptions {
    /// Capped at 12 to bound subset enumeration. Above the threshold, use greedy ordering.
    pub dp_limit: usize,
    /// Set only if the executor implements Leapfrog with the plan contract's variable order.
    pub enable_leapfrog: bool,
}
impl Default for PlannerOptions {
    fn default() -> Self {
        Self {
            dp_limit: 10,
            enable_leapfrog: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlanDecision {
    pub tree: JoinTree,
    pub search: &'static str,
    pub reason: &'static str,
    pub binary_cost: f64,
    /// Cost proxy from a feasible uniform fractional cover, using ESTIMATED scan sizes.
    /// Not a bound on actual data and not an implementation/performance guarantee.
    pub leapfrog_cost: Option<f64>,
}

#[derive(Clone)]
struct Candidate {
    tree: JoinTree,
    cost: f64,
    sorted: Option<u32>,
    vars: BTreeSet<u32>,
}

pub fn plan_bgp(patterns: &[TriplePattern], stats: &Stats) -> JoinTree {
    plan_bgp_with_options(patterns, stats, PlannerOptions::default()).tree
}

pub fn plan_bgp_with_options(
    patterns: &[TriplePattern],
    stats: &Stats,
    options: PlannerOptions,
) -> PlanDecision {
    if patterns.is_empty() {
        return decision(JoinTree::Unit, "identity", "empty BGP", 0.0, None);
    }
    if patterns.iter().any(|p| {
        let e = stats.estimate_pattern(p);
        e.exact && e.rows == 0.0
    }) {
        return decision(JoinTree::Empty, "empty", "exact empty scan", 0.0, None);
    }
    let (binary, search) = if patterns.len() <= options.dp_limit.min(12) {
        (dp(patterns, stats), "subset-dp")
    } else {
        (greedy(patterns, stats), "connected-greedy")
    };
    let eligible = patterns.len() >= 3
        && patterns.iter().all(|p| !has_repeated_variable(p))
        && cyclic(patterns);
    let proxy = if eligible {
        Some(leapfrog_proxy(patterns, stats))
    } else {
        None
    };
    if options.enable_leapfrog && proxy.is_some_and(|cost| cost < binary.cost) {
        let vars = variable_order(patterns, stats);
        let children = patterns
            .iter()
            .map(|p| JoinTree::Scan {
                pattern: *p,
                perm: trie_permutation(p, &vars),
                est: stats.estimate_pattern(p).rows,
            })
            .collect();
        return decision(
            JoinTree::Leapfrog {
                vars,
                children,
                est: stats.estimate_bgp(patterns),
            },
            search,
            "cyclic BGP; lower estimated trie cost",
            binary.cost,
            proxy,
        );
    }
    let reason = if eligible && !options.enable_leapfrog {
        "binary fallback: leapfrog executor disabled"
    } else if eligible {
        "binary cost no greater than trie proxy"
    } else {
        "acyclic/small/repeated-variable BGP"
    };
    decision(binary.tree, search, reason, binary.cost, proxy)
}

fn decision(
    tree: JoinTree,
    search: &'static str,
    reason: &'static str,
    binary_cost: f64,
    leapfrog_cost: Option<f64>,
) -> PlanDecision {
    PlanDecision {
        tree,
        search,
        reason,
        binary_cost,
        leapfrog_cost,
    }
}

fn scans(pattern: &TriplePattern, stats: &Stats) -> Vec<Candidate> {
    let mut by_sort = BTreeMap::new();
    for preferred in variables(pattern)
        .into_iter()
        .map(Some)
        .chain(std::iter::once(None))
    {
        let perm = scan_permutation(pattern, preferred);
        let sorted = scan_order(pattern, perm);
        let rows = stats.estimate_pattern(pattern).rows;
        by_sort.entry(sorted).or_insert(Candidate {
            tree: JoinTree::Scan {
                pattern: *pattern,
                perm,
                est: rows,
            },
            cost: rows,
            sorted,
            vars: variables(pattern),
        });
    }
    by_sort.into_values().collect()
}
fn scan_order(p: &TriplePattern, perm: Perm) -> Option<u32> {
    let s = slots(p);
    perm.order().into_iter().find_map(|i| {
        if let Slot::Var(v) = s[i] {
            Some(v)
        } else {
            None
        }
    })
}

fn joins(left: &Candidate, right: &Candidate, rows: f64) -> Vec<Candidate> {
    let shared: Vec<_> = left.vars.intersection(&right.vars).copied().collect();
    let vars = left.vars.union(&right.vars).copied().collect();
    let input = cardinal(left.tree.est() + right.tree.est());
    let common = cardinal(left.cost + right.cost + rows);
    let (build, probe) = if left.tree.est() <= right.tree.est() {
        (&left.tree, &right.tree)
    } else {
        (&right.tree, &left.tree)
    };
    let mut result = vec![Candidate {
        tree: JoinTree::HashJoin {
            vars: shared.clone(),
            build: Box::new(build.clone()),
            probe: Box::new(probe.clone()),
            est: rows,
        },
        cost: cardinal(common + 1.3 * input),
        sorted: None,
        vars,
    }];
    if shared.len() == 1 && left.sorted == Some(shared[0]) && right.sorted == Some(shared[0]) {
        result.push(Candidate {
            tree: JoinTree::MergeJoin {
                var: shared[0],
                left: Box::new(left.tree.clone()),
                right: Box::new(right.tree.clone()),
                est: rows,
            },
            cost: cardinal(common + input),
            sorted: Some(shared[0]),
            vars: result[0].vars.clone(),
        });
    }
    result
}
fn keep(table: &mut BTreeMap<Option<u32>, Candidate>, candidate: Candidate) {
    let replace = table
        .get(&candidate.sorted)
        .is_none_or(|old| candidate.cost < old.cost);
    if replace {
        table.insert(candidate.sorted, candidate);
    }
}
fn dp(patterns: &[TriplePattern], stats: &Stats) -> Candidate {
    let n = patterns.len();
    let full = 1usize << n;
    let leaves: Vec<_> = patterns.iter().map(|p| scans(p, stats)).collect();
    let mut states: Vec<BTreeMap<Option<u32>, Candidate>> =
        (0..full).map(|_| BTreeMap::new()).collect();
    for (i, leaf) in leaves.iter().enumerate() {
        for c in leaf {
            keep(&mut states[1 << i], c.clone());
        }
    }
    for mask in 1usize..full {
        if mask.count_ones() < 2 {
            continue;
        }
        let subset: Vec<_> = patterns
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, p)| *p)
            .collect();
        let rows = stats.estimate_bgp(&subset);
        let mut best = BTreeMap::new();
        for (i, leaf) in leaves.iter().enumerate() {
            if mask & (1 << i) == 0 {
                continue;
            }
            for left in states[mask ^ (1 << i)].values() {
                for right in leaf {
                    for c in joins(left, right, rows) {
                        keep(&mut best, c);
                    }
                }
            }
        }
        states[mask] = best;
    }
    states[full - 1]
        .values()
        .min_by(|a, b| a.cost.total_cmp(&b.cost))
        .unwrap()
        .clone()
}
fn greedy(patterns: &[TriplePattern], stats: &Stats) -> Candidate {
    let first = (0..patterns.len())
        .min_by(|a, b| {
            stats
                .estimate_pattern(&patterns[*a])
                .rows
                .total_cmp(&stats.estimate_pattern(&patterns[*b]).rows)
        })
        .unwrap();
    let mut chosen = vec![first];
    let mut current = scans(&patterns[first], stats);
    while chosen.len() < patterns.len() {
        let vars = &current[0].vars;
        let connected = (0..patterns.len())
            .any(|i| !chosen.contains(&i) && !vars.is_disjoint(&variables(&patterns[i])));
        let mut winner: Option<(usize, BTreeMap<Option<u32>, Candidate>, f64)> = None;
        for i in 0..patterns.len() {
            if chosen.contains(&i) || (connected && vars.is_disjoint(&variables(&patterns[i]))) {
                continue;
            }
            let subset: Vec<_> = chosen
                .iter()
                .copied()
                .chain(std::iter::once(i))
                .map(|j| patterns[j])
                .collect();
            let rows = stats.estimate_bgp(&subset);
            let mut candidates = BTreeMap::new();
            for left in &current {
                for right in scans(&patterns[i], stats) {
                    for c in joins(left, &right, rows) {
                        keep(&mut candidates, c);
                    }
                }
            }
            let cost = candidates
                .values()
                .map(|c| c.cost)
                .min_by(f64::total_cmp)
                .unwrap();
            if winner.as_ref().is_none_or(|(_, _, old)| cost < *old) {
                winner = Some((i, candidates, cost));
            }
        }
        let (i, candidates, _) = winner.unwrap();
        chosen.push(i);
        current = candidates.into_values().collect();
    }
    current
        .into_iter()
        .min_by(|a, b| a.cost.total_cmp(&b.cost))
        .unwrap()
}

/// GYO reduction on the variable hypergraph. An ordinary RDF star is acyclic,
/// even though its relation-intersection graph has many pairwise edges.
fn cyclic(patterns: &[TriplePattern]) -> bool {
    let mut edges: Vec<_> = patterns
        .iter()
        .map(variables)
        .filter(|e| !e.is_empty())
        .collect();
    loop {
        let previous = edges.clone();
        let mut removed = vec![false; edges.len()];
        for i in 0..edges.len() {
            for j in 0..edges.len() {
                if i != j
                    && !removed[j]
                    && edges[i].is_subset(&edges[j])
                    && (edges[i] != edges[j] || i > j)
                {
                    removed[i] = true;
                    break;
                }
            }
        }
        edges = edges
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !removed[*i])
            .map(|(_, e)| e)
            .collect();
        let mut frequency = BTreeMap::new();
        for e in &edges {
            for v in e {
                *frequency.entry(*v).or_insert(0) += 1;
            }
        }
        for e in &mut edges {
            e.retain(|v| frequency[v] > 1);
        }
        edges.retain(|e| !e.is_empty());
        if edges.is_empty() {
            return false;
        }
        if edges == previous {
            return true;
        }
    }
}
fn variable_order(patterns: &[TriplePattern], stats: &Stats) -> Vec<u32> {
    let domains = stats.variable_domains(patterns);
    let mut vars: Vec<_> = domains.keys().copied().collect();
    vars.sort_by(|a, b| domains[a].total_cmp(&domains[b]).then_with(|| a.cmp(b)));
    vars
}
fn trie_permutation(p: &TriplePattern, vars: &[u32]) -> Perm {
    let s = slots(p);
    let mut wanted = [0, 1, 2];
    wanted.sort_by_key(|i| match s[*i] {
        Slot::Const(_) => (0, 0, *i),
        Slot::Var(v) => (1, vars.iter().position(|x| *x == v).unwrap(), *i),
    });
    Perm::ALL.into_iter().find(|p| p.order() == wanted).unwrap()
}
fn leapfrog_proxy(patterns: &[TriplePattern], stats: &Stats) -> f64 {
    let mut degrees = BTreeMap::new();
    let mut log_product = 0.0;
    let mut scan_work = 0.0;
    for p in patterns {
        let n = stats.estimate_pattern(p).rows.max(1.0);
        log_product += n.ln();
        scan_work += n;
        for v in variables(p) {
            *degrees.entry(v).or_insert(0usize) += 1;
        }
    }
    let min_degree = degrees.values().copied().min().unwrap_or(1) as f64;
    let bound = (log_product / min_degree).min(1e300f64.ln()).exp();
    cardinal(scan_work + bound * 1.5)
}
