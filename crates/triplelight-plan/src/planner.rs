//! Left-deep, order-aware subset DP for small BGPs, connected greedy extension for large BGPs.
use super::plan::{JoinTree, Slot, TriplePattern};
use super::stats::{Stats, cardinal, has_repeated_variable, scan_permutation, slots, variables};
use crate::model::Perm;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryJoinMode {
    /// P1 executor: each outer binding performs an indexed lookup.
    IndexNestedLoop,
    /// For an executor that actually materializes merge/hash inputs.
    Materialized,
}

#[derive(Clone, Copy, Debug)]
pub struct PlannerOptions {
    /// Capped at 12 to bound subset enumeration. Above the threshold, use greedy ordering.
    pub dp_limit: usize,
    /// Set only if the executor implements Leapfrog with the plan contract's variable order.
    pub enable_leapfrog: bool,
    pub binary_mode: BinaryJoinMode,
}
impl Default for PlannerOptions {
    fn default() -> Self {
        Self {
            dp_limit: 10,
            enable_leapfrog: false,
            binary_mode: BinaryJoinMode::IndexNestedLoop,
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

/// Work gate in approximate index seeks plus matched rows, not nanoseconds.
pub const MIN_OPTIMIZER_WORK: f64 = 100_000.0;
/// Runtime stats optimization is limited to 128 binary-DP subset states.
/// Wider queries retain the executor's original index-count greedy/trie order.
pub const MAX_RUNTIME_DP_PATTERNS: usize = 7;

/// Cheap greedy work proxy: independent per-pattern domains, no CS enumeration,
/// data reads or subset search. A wrong estimate affects strategy, never answers.
pub fn estimated_greedy_work(patterns: &[TriplePattern], stats: &Stats) -> f64 {
    let domains: Vec<_> = patterns
        .iter()
        .map(|p| stats.variable_domains(&[*p]))
        .collect();
    let sizes: Vec<_> = patterns
        .iter()
        .map(|p| stats.estimate_pattern(p).rows)
        .collect();
    let mut used = vec![false; patterns.len()];
    let mut bound = BTreeSet::new();
    let mut rows = 1.0;
    let mut work = 0.0;
    for _ in patterns {
        let connected =
            (0..patterns.len()).any(|i| !used[i] && domains[i].keys().any(|v| bound.contains(v)));
        let mut best: Option<(usize, f64)> = None;
        for i in 0..patterns.len() {
            if used[i] || (connected && !domains[i].keys().any(|v| bound.contains(v))) {
                continue;
            }
            let mut fanout = sizes[i];
            for (v, ndv) in &domains[i] {
                if bound.contains(v) {
                    fanout /= ndv.min(sizes[i].max(1.0)).max(1.0);
                }
            }
            if best.is_none_or(|(_, old)| fanout < old) {
                best = Some((i, fanout));
            }
        }
        let (i, fanout) = best.unwrap();
        used[i] = true;
        let next = cardinal(rows * fanout);
        work = cardinal(work + rows + next);
        rows = next;
        bound.extend(domains[i].keys().copied());
    }
    work
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
    let compiled =
        (patterns.len() <= options.dp_limit.min(12)).then(|| stats.for_planning(patterns));
    let stats = compiled.as_ref().unwrap_or(stats);
    let (binary, search) = if patterns.len() <= options.dp_limit.min(12) {
        (
            if options.binary_mode == BinaryJoinMode::IndexNestedLoop {
                dp_index(patterns, stats)
            } else {
                dp(patterns, stats, options.binary_mode)
            },
            "subset-dp",
        )
    } else {
        (
            greedy(patterns, stats, options.binary_mode),
            "connected-greedy",
        )
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

fn scans(pattern: &TriplePattern, stats: &Stats, mode: BinaryJoinMode) -> Vec<Candidate> {
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
            cost: rows
                + if mode == BinaryJoinMode::IndexNestedLoop {
                    index_seek_cost(stats)
                } else {
                    0.0
                },
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

fn joins(
    left: &Candidate,
    right: &Candidate,
    rows: f64,
    stats: &Stats,
    mode: BinaryJoinMode,
) -> Vec<Candidate> {
    if mode == BinaryJoinMode::IndexNestedLoop {
        let JoinTree::Scan { pattern, .. } = &right.tree else {
            unreachable!("left-deep inner must be a scan")
        };
        let perm = bind_permutation(pattern, &left.vars);
        // rows_out = rows_in * conditional fanout; avoid unstable division when rows_in is zero.
        let cost = cardinal(left.cost + left.tree.est() * index_seek_cost(stats) + rows);
        return vec![Candidate {
            tree: JoinTree::IndexNestedLoop {
                outer: Box::new(left.tree.clone()),
                pattern: *pattern,
                perm,
                est: rows,
            },
            cost,
            sorted: None,
            vars: left.vars.union(&right.vars).copied().collect(),
        }];
    }
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
// Index nested loops discard ordering after a join. Their cost depends only on
// the outer subset's cardinality, not on a retained physical order. Keep compact
// predecessor/cost arrays and build the winning tree once instead of cloning a
// boxed tree for every losing transition.
fn dp_index(patterns: &[TriplePattern], stats: &Stats) -> Candidate {
    let full = 1usize << patterns.len();
    let mut costs = vec![f64::INFINITY; full];
    let mut cardinalities = vec![0.0; full];
    let mut last = vec![0usize; full];
    let seek = index_seek_cost(stats);
    let leaves: Vec<_> = patterns
        .iter()
        .map(|p| {
            scans(p, stats, BinaryJoinMode::IndexNestedLoop)
                .into_iter()
                .min_by(|a, b| a.cost.total_cmp(&b.cost))
                .unwrap()
        })
        .collect();
    for (i, leaf) in leaves.iter().enumerate() {
        costs[1 << i] = leaf.cost;
        cardinalities[1 << i] = leaf.tree.est();
        last[1 << i] = i;
    }
    let mut subset = Vec::with_capacity(patterns.len());
    for mask in 1usize..full {
        if mask.is_power_of_two() {
            continue;
        }
        subset.clear();
        subset.extend(
            patterns
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, p)| *p),
        );
        cardinalities[mask] = stats.estimate_bgp(&subset);
        let mut remaining = mask;
        while remaining != 0 {
            let i = remaining.trailing_zeros() as usize;
            remaining &= remaining - 1;
            let outer = mask ^ (1 << i);
            let cost = cardinal(costs[outer] + cardinalities[outer] * seek + cardinalities[mask]);
            if cost < costs[mask] {
                costs[mask] = cost;
                last[mask] = i;
            }
        }
    }
    let mut order = Vec::with_capacity(patterns.len());
    let mut mask = full - 1;
    while mask != 0 {
        let i = last[mask];
        order.push(i);
        mask ^= 1 << i;
    }
    order.reverse();
    let mut winner = leaves[order[0]].clone();
    mask = 1 << order[0];
    for &i in &order[1..] {
        mask |= 1 << i;
        winner.tree = JoinTree::IndexNestedLoop {
            perm: bind_permutation(&patterns[i], &winner.vars),
            outer: Box::new(winner.tree),
            pattern: patterns[i],
            est: cardinalities[mask],
        };
        winner.vars.extend(variables(&patterns[i]));
        winner.sorted = None;
    }
    winner.cost = costs[full - 1];
    winner
}

fn dp(patterns: &[TriplePattern], stats: &Stats, mode: BinaryJoinMode) -> Candidate {
    let n = patterns.len();
    let full = 1usize << n;
    let leaves: Vec<_> = patterns.iter().map(|p| scans(p, stats, mode)).collect();
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
                    for c in joins(left, right, rows, stats, mode) {
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
fn greedy(patterns: &[TriplePattern], stats: &Stats, mode: BinaryJoinMode) -> Candidate {
    let first = (0..patterns.len())
        .min_by(|a, b| {
            stats
                .estimate_pattern(&patterns[*a])
                .rows
                .total_cmp(&stats.estimate_pattern(&patterns[*b]).rows)
        })
        .unwrap();
    let mut chosen = vec![first];
    let mut current = scans(&patterns[first], stats, mode);
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
                for right in scans(&patterns[i], stats, mode) {
                    for c in joins(left, &right, rows, stats, mode) {
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
    choose_variable_order(patterns, stats)
}

/// Minimize a uniform-domain projection/seek work estimate. This is a heuristic,
/// not an AGM bound or a guarantee about elapsed time. Up to 12 variables use
/// subset DP; larger queries use greedy prefix extension.
pub fn choose_variable_order(patterns: &[TriplePattern], stats: &Stats) -> Vec<u32> {
    let model = PrefixModel::new(patterns, stats);
    let n = model.vars.len();
    if n == 0 {
        return Vec::new();
    }
    if n <= 12 {
        let full = 1usize << n;
        let mut cost = vec![f64::INFINITY; full];
        let mut last = vec![0usize; full];
        cost[0] = 0.0;
        for mask in 1usize..full {
            let selected: Vec<_> = (0..n).filter(|i| mask & (1 << i) != 0).collect();
            let rows = model.rows(&selected);
            for &i in &selected {
                let outer = mask ^ (1 << i);
                let candidate = cardinal(cost[outer] + rows * model.degree[i] * model.seek);
                if candidate < cost[mask] {
                    cost[mask] = candidate;
                    last[mask] = i;
                }
            }
        }
        let mut order = Vec::with_capacity(n);
        let mut mask = full - 1;
        while mask != 0 {
            let i = last[mask];
            order.push(model.vars[i]);
            mask ^= 1 << i;
        }
        order.reverse();
        order
    } else {
        let mut selected = Vec::with_capacity(n);
        while selected.len() < n {
            let i = (0..n)
                .filter(|i| !selected.contains(i))
                .min_by(|a, b| {
                    let mut trial = selected.clone();
                    trial.push(*a);
                    let ca = cardinal(model.rows(&trial) * model.degree[*a] * model.seek);
                    *trial.last_mut().unwrap() = *b;
                    let cb = cardinal(model.rows(&trial) * model.degree[*b] * model.seek);
                    ca.total_cmp(&cb).then(a.cmp(b))
                })
                .unwrap();
            selected.push(i);
        }
        selected.into_iter().map(|i| model.vars[i]).collect()
    }
}

/// Estimated work for a complete permutation; None rejects missing, extra or
/// repeated variables instead of silently scoring an invalid trie order.
pub fn variable_order_work(
    patterns: &[TriplePattern],
    stats: &Stats,
    order: &[u32],
) -> Option<f64> {
    let model = PrefixModel::new(patterns, stats);
    if order.len() != model.vars.len() {
        return None;
    }
    let mut selected = Vec::with_capacity(order.len());
    let mut work = 0.0;
    for v in order {
        let i = model.vars.binary_search(v).ok()?;
        if selected.contains(&i) {
            return None;
        }
        selected.push(i);
        work = cardinal(work + model.rows(&selected) * model.degree[i] * model.seek);
    }
    Some(work)
}
struct PrefixRelation {
    rows: f64,
    domains: Vec<(usize, f64)>,
}
struct PrefixModel {
    vars: Vec<u32>,
    relations: Vec<PrefixRelation>,
    degree: Vec<f64>,
    seek: f64,
}
impl PrefixModel {
    fn new(patterns: &[TriplePattern], stats: &Stats) -> Self {
        let vars: Vec<_> = patterns
            .iter()
            .flat_map(variables)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut degree = vec![0.0; vars.len()];
        let mut relations = Vec::new();
        let mut unique = Vec::new();
        for p in patterns {
            if unique.contains(p) {
                continue;
            }
            unique.push(*p);
            let rows = stats.estimate_pattern(p).rows;
            let domains = stats
                .variable_domains(&[*p])
                .into_iter()
                .map(|(v, d)| {
                    let i = vars.binary_search(&v).unwrap();
                    degree[i] += 1.0;
                    (i, d.min(rows.max(1.0)))
                })
                .collect();
            relations.push(PrefixRelation { rows, domains });
        }
        Self {
            vars,
            relations,
            degree,
            seek: index_seek_cost(stats),
        }
    }
    fn rows(&self, selected: &[usize]) -> f64 {
        let mut joined = 1.0;
        let mut seen = vec![None::<f64>; self.vars.len()];
        for relation in &self.relations {
            if relation.rows == 0.0 {
                return 0.0;
            }
            let present: Vec<_> = relation
                .domains
                .iter()
                .filter(|(i, _)| selected.contains(i))
                .collect();
            if present.is_empty() {
                continue;
            }
            let projected = present
                .iter()
                .fold(1.0, |n, (_, d)| cardinal(n * d))
                .min(relation.rows);
            joined = cardinal(joined * projected);
            for &&(i, d) in &present {
                let domain = d.min(projected.max(1.0));
                if let Some(prior) = seen[i] {
                    joined /= prior.max(domain);
                    seen[i] = Some(prior.min(domain));
                } else {
                    seen[i] = Some(domain);
                }
            }
        }
        cardinal(joined)
    }
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

/// In-memory sorted-index seek proxy. This is not a measured nanosecond cost.
fn index_seek_cost(stats: &Stats) -> f64 {
    (stats.triple_count.max(2) as f64).log2()
}
fn bind_permutation(pattern: &TriplePattern, bound: &BTreeSet<u32>) -> Perm {
    let s = slots(pattern);
    Perm::ALL
        .into_iter()
        .max_by_key(|perm| {
            perm.order()
                .iter()
                .take_while(|i| match s[**i] {
                    Slot::Const(_) => true,
                    Slot::Var(v) => bound.contains(&v),
                })
                .count()
        })
        .unwrap()
}

#[cfg(test)]
mod compact_dp_tests {
    use super::*;
    use crate::model::TermId;
    #[test]
    fn compact_index_dp_matches_ordered_reference_cost_and_tree() {
        for seed in 0..24u64 {
            let mut triples: Vec<_> = (0..40u64)
                .map(|i| {
                    [
                        TermId(i % 9),
                        TermId(10 + (i + seed) % 4),
                        TermId((i * 7 + seed) % 13),
                    ]
                })
                .collect();
            triples.sort();
            triples.dedup();
            let stats = Stats::from_spo(triples).unwrap();
            let patterns: Vec<_> = (0..6u32)
                .map(|i| TriplePattern {
                    s: Slot::Var(i % 3),
                    p: Slot::Const(TermId(10 + (u64::from(i) + seed) % 4)),
                    o: if i == 5 {
                        Slot::Const(TermId(seed % 13))
                    } else {
                        Slot::Var((i + 1) % 4)
                    },
                })
                .collect();
            for n in 1..=patterns.len() {
                let fast = dp_index(&patterns[..n], &stats);
                let reference = dp(&patterns[..n], &stats, BinaryJoinMode::IndexNestedLoop);
                assert_eq!(fast.cost, reference.cost, "seed={seed}, n={n}");
                assert_eq!(
                    format!("{:?}", fast.tree),
                    format!("{:?}", reference.tree),
                    "seed={seed}, n={n}"
                );
            }
        }
    }
}
