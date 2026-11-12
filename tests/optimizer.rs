use oxrdf::Term;
use rdfdb::model::{Perm, TermId, TripleBatch};
use rdfdb::opt::plan::{JoinTree, Slot, TriplePattern};
use rdfdb::opt::stats::scan_permutation;
use rdfdb::opt::{PlannerOptions, Stats, StatsError, plan_bgp, plan_bgp_with_options, q_error};
use rdfdb::storage::{Dictionary, Snapshot};
use std::collections::{BTreeMap, BTreeSet};
fn c(x: u64) -> Slot {
    Slot::Const(TermId(x))
}
fn v(x: u32) -> Slot {
    Slot::Var(x)
}
fn p(s: Slot, p: Slot, o: Slot) -> TriplePattern {
    TriplePattern { s, p, o }
}
fn ids(t: [u64; 3]) -> [TermId; 3] {
    t.map(TermId)
}
struct Graph {
    triples: Vec<[TermId; 3]>,
}
impl Graph {
    fn new(triples: Vec<[u64; 3]>) -> Self {
        let mut triples: Vec<_> = triples.into_iter().map(ids).collect();
        triples.sort();
        triples.dedup();
        Self { triples }
    }
    fn stats(&self) -> Stats {
        Stats::from_spo(self.triples.iter().copied()).unwrap()
    }
}
impl Dictionary for Graph {
    fn lookup(&self, _: &Term) -> Option<TermId> {
        None
    }
    fn resolve(&self, _: TermId) -> Term {
        panic!("not used")
    }
}
impl Snapshot for Graph {
    fn scan<'a>(
        &'a self,
        perm: Perm,
        prefix: &[TermId],
    ) -> Box<dyn Iterator<Item = TripleBatch> + 'a> {
        let order = perm.order();
        let mut rows: Vec<_> = self
            .triples
            .iter()
            .map(|t| order.map(|i| t[i]))
            .filter(|r| r.starts_with(prefix))
            .collect();
        rows.sort();
        let mut b = TripleBatch::default();
        for row in rows {
            for (i, id) in row.into_iter().enumerate() {
                b.cols[i].push(id);
            }
        }
        Box::new(std::iter::once(b))
    }
    fn count(&self, perm: Perm, prefix: &[TermId]) -> u64 {
        self.scan(perm, prefix).map(|b| b.len() as u64).sum()
    }
    fn seek(&self, perm: Perm, prefix: &[TermId], target: TermId) -> Option<TermId> {
        self.scan(perm, prefix)
            .flat_map(|b| b.cols[prefix.len()].clone())
            .find(|id| *id >= target)
    }
}
type Binding = BTreeMap<u32, TermId>;
fn scan(g: &Graph, p: &TriplePattern) -> Vec<Binding> {
    g.triples
        .iter()
        .filter_map(|t| {
            let mut b = Binding::new();
            for (slot, id) in [p.s, p.p, p.o].into_iter().zip(t) {
                match slot {
                    Slot::Const(want) if want != *id => return None,
                    Slot::Var(var) => {
                        if b.get(&var).is_some_and(|old| old != id) {
                            return None;
                        }
                        b.insert(var, *id);
                    }
                    _ => {}
                }
            }
            Some(b)
        })
        .collect()
}
fn join(a: Vec<Binding>, b: Vec<Binding>) -> Vec<Binding> {
    let mut out = Vec::new();
    for left in a {
        for right in &b {
            if left
                .iter()
                .all(|(k, v)| right.get(k).is_none_or(|w| w == v))
            {
                let mut row = left.clone();
                row.extend(right);
                out.push(row);
            }
        }
    }
    out
}
fn truth(g: &Graph, patterns: &[TriplePattern]) -> Vec<Binding> {
    patterns
        .iter()
        .fold(vec![Binding::new()], |r, p| join(r, scan(g, p)))
}
// Independent relational interpretation of the emitted plan, not a performance executor.
fn eval(g: &Graph, t: &JoinTree) -> Vec<Binding> {
    match t {
        JoinTree::Unit => vec![Binding::new()],
        JoinTree::Empty => vec![],
        JoinTree::Scan { pattern, .. } => scan(g, pattern),
        JoinTree::MergeJoin { left, right, .. } => join(eval(g, left), eval(g, right)),
        JoinTree::HashJoin { build, probe, .. } => join(eval(g, build), eval(g, probe)),
        JoinTree::Leapfrog { children, .. } => children
            .iter()
            .fold(vec![Binding::new()], |r, c| join(r, eval(g, c))),
    }
}
fn all_vars(t: &JoinTree) -> BTreeSet<u32> {
    match t {
        JoinTree::Unit | JoinTree::Empty => BTreeSet::new(),
        JoinTree::Scan { pattern, .. } => [pattern.s, pattern.p, pattern.o]
            .into_iter()
            .filter_map(|s| if let Slot::Var(v) = s { Some(v) } else { None })
            .collect(),
        JoinTree::MergeJoin { left, right, .. } => {
            all_vars(left).union(&all_vars(right)).copied().collect()
        }
        JoinTree::HashJoin { build, probe, .. } => {
            all_vars(build).union(&all_vars(probe)).copied().collect()
        }
        JoinTree::Leapfrog { children, .. } => children.iter().flat_map(all_vars).collect(),
    }
}
fn sorted(t: &JoinTree) -> Option<u32> {
    match t {
        JoinTree::Scan { pattern, perm, .. } => {
            let s = [pattern.s, pattern.p, pattern.o];
            perm.order().into_iter().find_map(|i| {
                if let Slot::Var(v) = s[i] {
                    Some(v)
                } else {
                    None
                }
            })
        }
        JoinTree::MergeJoin { var, .. } => Some(*var),
        _ => None,
    }
}
fn validate_plan(t: &JoinTree) {
    assert!(t.est().is_finite() && t.est() >= 0.0);
    match t {
        JoinTree::MergeJoin {
            var, left, right, ..
        } => {
            assert_eq!(sorted(left), Some(*var));
            assert_eq!(sorted(right), Some(*var));
            assert_eq!(
                all_vars(left)
                    .intersection(&all_vars(right))
                    .copied()
                    .collect::<Vec<_>>(),
                vec![*var]
            );
            validate_plan(left);
            validate_plan(right);
        }
        JoinTree::HashJoin {
            vars, build, probe, ..
        } => {
            assert_eq!(
                *vars,
                all_vars(build)
                    .intersection(&all_vars(probe))
                    .copied()
                    .collect::<Vec<_>>()
            );
            validate_plan(build);
            validate_plan(probe);
        }
        JoinTree::Leapfrog { vars, children, .. } => {
            for child in children {
                let JoinTree::Scan { pattern, perm, .. } = child else {
                    panic!("trie child is not scan")
                };
                let s = [pattern.s, pattern.p, pattern.o];
                let ranks: Vec<_> = perm
                    .order()
                    .into_iter()
                    .filter_map(|i| {
                        if let Slot::Var(v) = s[i] {
                            Some(vars.iter().position(|x| *x == v).unwrap())
                        } else {
                            None
                        }
                    })
                    .collect();
                assert!(ranks.windows(2).all(|w| w[0] <= w[1]));
                validate_plan(child);
            }
        }
        _ => {}
    }
}
#[test]
fn spo_stream_deduplicates_and_rejects_unsorted() {
    let s = Stats::from_spo([ids([1, 10, 2]), ids([1, 10, 2]), ids([2, 10, 3])]).unwrap();
    assert_eq!(s.triple_count, 2);
    assert_eq!(s.subject_count, 2);
    assert_eq!(s.duplicate_triples_ignored, 1);
    assert!(matches!(
        Stats::from_spo([ids([2, 10, 3]), ids([1, 10, 2])]),
        Err(StatsError::UnsortedSpo)
    ));
}
#[test]
fn snapshot_stats_and_exact_counts() {
    let g = Graph::new(vec![[1, 10, 2], [1, 10, 3], [2, 11, 4]]);
    let mut s = Stats::from_snapshot(&g).unwrap();
    assert_eq!(s.triple_count, 3);
    let qs = [
        p(c(1), c(10), v(0)),
        p(c(999), c(10), v(0)),
        p(c(1), c(10), c(2)),
    ];
    s.cache_pattern_counts(&qs, &g);
    for q in qs {
        let e = s.estimate_pattern(&q);
        assert!(e.exact);
        assert_eq!(e.rows, scan(&g, &q).len() as f64);
    }
    assert!(matches!(plan_bgp(&qs[1..2], &s), JoinTree::Empty));
}
#[test]
fn correlated_star_uses_characteristic_sets() {
    let mut triples = Vec::new();
    for i in 1..=100 {
        triples.push([i, 1000, 2000 + i]);
        if i <= 50 {
            triples.push([i, 1001, 3000 + i]);
        }
    }
    let g = Graph::new(triples);
    let s = g.stats();
    let q = [p(v(0), c(1000), v(1)), p(v(0), c(1001), v(2))];
    assert_eq!(s.characteristic_sets.len(), 2);
    assert_eq!(s.estimate_bgp(&q), 50.0);
    assert_eq!(truth(&g, &q).len(), 50);
}
#[test]
fn cs_multiplicity_is_an_estimate_not_exact_degree_correlation() {
    let g = Graph::new(vec![
        [1, 10, 20],
        [1, 10, 21],
        [1, 11, 30],
        [1, 11, 31],
        [1, 11, 32],
        [2, 10, 22],
        [2, 11, 33],
    ]);
    let q = [p(v(0), c(10), v(1)), p(v(0), c(11), v(2))];
    let estimate = g.stats().estimate_bgp(&q);
    let actual = truth(&g, &q).len() as u64;
    assert_eq!(estimate, 6.0);
    assert_eq!(actual, 7);
    assert!((q_error(estimate, actual) - 7.0 / 6.0).abs() < 1e-10);
}
#[test]
fn bound_objects_exact_before_frequency_overflow() {
    let g = Graph::new(vec![[1, 10, 7], [2, 10, 7], [3, 10, 8]]);
    let s = g.stats();
    let a = s.estimate_pattern(&p(v(0), c(10), c(7)));
    assert!(a.exact);
    assert_eq!(a.rows, 2.0);
    let missing = p(v(0), c(10), c(99));
    assert!(matches!(plan_bgp(&[missing], &s), JoinTree::Empty));
}
#[test]
fn histogram_overflow_never_proves_absence() {
    let g = Graph::new((1..=1000).map(|i| [i, 10, 10000 + i]).collect());
    let s = g.stats();
    let q = p(v(0), c(10), c(999999));
    let e = s.estimate_pattern(&q);
    assert!(!e.exact);
    assert!(e.rows > 0.0);
    assert!(!matches!(plan_bgp(&[q], &s), JoinTree::Empty));
}
#[test]
fn repeated_variable_scans_use_equality_counts() {
    let g = Graph::new(vec![[1, 1, 1], [2, 3, 2], [4, 4, 5]]);
    let s = g.stats();
    for q in [
        p(v(0), v(1), v(0)),
        p(v(0), v(0), v(0)),
        p(v(0), v(0), v(1)),
    ] {
        let e = s.estimate_pattern(&q);
        assert!(e.exact);
        assert_eq!(e.rows, scan(&g, &q).len() as f64);
    }
}
#[test]
fn all_constant_masks_get_maximal_index_prefix() {
    for mask in 0usize..8 {
        let s: [Slot; 3] = std::array::from_fn(|i| {
            if mask & (1 << i) > 0 {
                c((10 + i) as u64)
            } else {
                v(i as u32)
            }
        });
        let q = p(s[0], s[1], s[2]);
        let perm = scan_permutation(&q, None);
        let prefix = perm
            .order()
            .iter()
            .take_while(|i| matches!(s[**i], Slot::Const(_)))
            .count();
        assert_eq!(prefix, mask.count_ones() as usize);
    }
}
#[test]
fn empty_bgp_has_one_binding_even_on_empty_graph() {
    assert!(matches!(plan_bgp(&[], &Stats::default()), JoinTree::Unit));
    assert_eq!(plan_bgp(&[], &Stats::default()).est(), 1.0);
}
#[test]
fn joins_preserve_all_shared_keys_and_physical_sort_property() {
    let g = Graph::new(vec![[1, 10, 2], [1, 10, 3], [1, 11, 3], [1, 11, 4]]);
    let s = g.stats();
    let q = [p(v(0), c(10), v(1)), p(v(0), c(11), v(1))];
    let t = plan_bgp(&q, &s);
    validate_plan(&t);
    assert!(matches!(&t,JoinTree::HashJoin{vars,..}if vars==&vec![0,1]));
    assert_eq!(eval(&g, &t), truth(&g, &q));
    let star = [p(v(0), c(10), v(1)), p(v(0), c(11), v(2))];
    let t = plan_bgp(&star, &s);
    validate_plan(&t);
    assert!(matches!(t, JoinTree::MergeJoin { .. }));
}
#[test]
fn cyclic_choice_is_capability_gated_and_stars_are_not_cycles() {
    let mut triples = Vec::new();
    for x in 1..=8 {
        triples.push([x, 100, 1000]);
        for z in 2000..2008 {
            triples.push([z, 102, x]);
        }
    }
    for z in 2000..2008 {
        triples.push([1000, 101, z]);
    }
    let g = Graph::new(triples);
    let s = g.stats();
    let cycle = [
        p(v(0), c(100), v(1)),
        p(v(1), c(101), v(2)),
        p(v(2), c(102), v(0)),
    ];
    let off = plan_bgp_with_options(&cycle, &s, PlannerOptions::default());
    assert!(off.leapfrog_cost.is_some());
    assert!(!matches!(off.tree, JoinTree::Leapfrog { .. }));
    let on = plan_bgp_with_options(
        &cycle,
        &s,
        PlannerOptions {
            enable_leapfrog: true,
            ..Default::default()
        },
    );
    assert!(
        matches!(on.tree, JoinTree::Leapfrog { .. }),
        "binary={} trie={:?}",
        on.binary_cost,
        on.leapfrog_cost
    );
    validate_plan(&on.tree);
    let mut a = eval(&g, &on.tree);
    let mut b = truth(&g, &cycle);
    a.sort();
    b.sort();
    assert_eq!(a, b);
    let star = [
        p(v(0), c(100), v(1)),
        p(v(0), c(101), v(2)),
        p(v(0), c(102), v(3)),
    ];
    let d = plan_bgp_with_options(
        &star,
        &s,
        PlannerOptions {
            enable_leapfrog: true,
            ..Default::default()
        },
    );
    assert!(d.leapfrog_cost.is_none());
}
#[test]
fn dp_compares_favorably_to_greedy_and_large_queries_fall_back() {
    let mut triples = Vec::new();
    for s in 1..=5 {
        for pred in 10..30 {
            triples.push([s, pred, s + pred]);
        }
    }
    let g = Graph::new(triples);
    let s = g.stats();
    let qs: Vec<_> = (10..30)
        .map(|pred| p(v(0), c(pred), v(pred as u32)))
        .collect();
    let dp = plan_bgp_with_options(&qs[..5], &s, PlannerOptions::default());
    let greedy = plan_bgp_with_options(
        &qs[..5],
        &s,
        PlannerOptions {
            dp_limit: 0,
            ..Default::default()
        },
    );
    assert!(dp.binary_cost <= greedy.binary_cost + 1e-8);
    assert_eq!(dp.search, "subset-dp");
    let large = plan_bgp_with_options(&qs, &s, PlannerOptions::default());
    assert_eq!(large.search, "connected-greedy");
    validate_plan(&large.tree);
}
#[test]
fn randomized_plan_semantics_match_independent_bgp_oracle() {
    let mut rng = 7u64;
    let mut next = || {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        rng >> 32
    };
    for _ in 0..60 {
        let triples: Vec<_> = (0..14)
            .map(|_| [next() % 6 + 1, next() % 3 + 10, next() % 6 + 1])
            .collect();
        let g = Graph::new(triples);
        let s = g.stats();
        let mut qs = Vec::new();
        for _ in 0..(next() % 4 + 1) {
            let a = if next() % 4 == 0 {
                c(next() % 6 + 1)
            } else {
                v((next() % 4) as u32)
            };
            let pred = if next() % 4 == 0 {
                v((next() % 4) as u32)
            } else {
                c(next() % 4 + 10)
            };
            let b = if next() % 4 == 0 {
                c(next() % 6 + 1)
            } else {
                v((next() % 4) as u32)
            };
            qs.push(p(a, pred, b));
        }
        let mut expected = truth(&g, &qs);
        expected.sort();
        for dp_limit in [0, 10] {
            let d = plan_bgp_with_options(
                &qs,
                &s,
                PlannerOptions {
                    dp_limit,
                    enable_leapfrog: true,
                },
            );
            validate_plan(&d.tree);
            let mut actual = eval(&g, &d.tree);
            actual.sort();
            assert_eq!(actual, expected, "query={qs:?}");
        }
    }
}
#[test]
fn zero_aware_q_error_reports_false_zeros() {
    assert_eq!(q_error(0., 0), 1.);
    assert_eq!(q_error(4., 2), 2.);
    assert!(q_error(0., 2).is_infinite());
    assert!(q_error(2., 0).is_infinite());
    assert!(q_error(f64::NAN, 2).is_infinite());
}

#[test]
fn bounded_pos_refinement_repairs_rare_object_stars() {
    let mut triples = Vec::new();
    for i in 1..=1000 {
        triples.push([i, 10, 10000 + i]);
        triples.push([i, 11, 20000 + i]);
    }
    let g = Graph::new(triples);
    let mut stats = g.stats();
    let q = [p(v(0), c(10), c(10001)), p(v(0), c(11), v(1))];
    assert!(stats.estimate_bgp(&q) > 1.0);
    stats.cache_pattern_counts(&q, &g);
    assert_eq!(stats.estimate_bgp(&q), 1.0);
    assert_eq!(truth(&g, &q).len(), 1);
}
#[test]
fn fixed_subject_star_uses_exact_leaf_products() {
    let g = Graph::new(vec![
        [1, 10, 20],
        [1, 10, 21],
        [1, 11, 30],
        [1, 11, 31],
        [1, 11, 32],
        [2, 10, 22],
        [2, 11, 33],
    ]);
    let mut stats = g.stats();
    let q = [p(c(1), c(10), v(0)), p(c(1), c(11), v(1))];
    stats.cache_pattern_counts(&q, &g);
    assert_eq!(stats.estimate_bgp(&q), 6.0);
    assert_eq!(truth(&g, &q).len(), 6);
}
