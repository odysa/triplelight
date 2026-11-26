use triplelight_rdf::Term;
use triplelight::model::{Perm, TermId, TripleBatch};
use triplelight::opt::plan::{JoinTree, Slot, TriplePattern};
use triplelight::opt::stats::scan_permutation;
use triplelight::opt::{
    BinaryJoinMode, PlannerOptions, Stats, StatsError, plan_bgp, plan_bgp_with_options, q_error,
};
use triplelight::storage::{Dictionary, Snapshot};
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
    fn lookup(&self, _: &Term) -> std::io::Result<Option<TermId>> {
        Ok(None)
    }
    fn resolve(&self, _: TermId) -> std::io::Result<Term> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "encoded-only graph fixture",
        ))
    }
    fn with_term(&self, _: TermId, _: &mut dyn FnMut(triplelight_rdf::TermRef<'_>)) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "encoded-only graph fixture",
        ))
    }
}
impl Snapshot for Graph {
    fn scan<'a>(
        &'a self,
        perm: Perm,
        prefix: &[TermId],
    ) -> Box<dyn Iterator<Item = Result<TripleBatch, std::io::Error>> + 'a> {
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
        Box::new(std::iter::once(Ok(b)))
    }
    fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, std::io::Error> {
        self.scan(perm, prefix)
            .map(|b| b.map(|b| b.len() as u64))
            .sum()
    }
    fn seek(
        &self,
        perm: Perm,
        prefix: &[TermId],
        target: TermId,
    ) -> Result<Option<TermId>, std::io::Error> {
        Ok(self
            .scan(perm, prefix)
            .flat_map(|b| b.unwrap().cols[prefix.len()].clone())
            .find(|id| *id >= target))
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
        JoinTree::IndexNestedLoop { outer, pattern, .. } => join(eval(g, outer), scan(g, pattern)),
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
        JoinTree::IndexNestedLoop { outer, pattern, .. } => {
            let inner: BTreeSet<_> = [pattern.s, pattern.p, pattern.o]
                .into_iter()
                .filter_map(|s| if let Slot::Var(v) = s { Some(v) } else { None })
                .collect();
            all_vars(outer).union(&inner).copied().collect()
        }
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
        JoinTree::IndexNestedLoop {
            outer,
            pattern,
            perm,
            ..
        } => {
            let bound = all_vars(outer);
            let slots = [pattern.s, pattern.p, pattern.o];
            let fixed = |i: usize| match slots[i] {
                Slot::Const(_) => true,
                Slot::Var(v) => bound.contains(&v),
            };
            let prefix = perm.order().iter().take_while(|i| fixed(**i)).count();
            assert_eq!(prefix, (0..3).filter(|i| fixed(*i)).count());
            validate_plan(outer);
        }
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
    s.cache_pattern_counts(&qs, &g).unwrap();
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
    assert_eq!(s.characteristic_set_count(), 2);
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
    let t = plan_bgp_with_options(
        &q,
        &s,
        PlannerOptions {
            binary_mode: BinaryJoinMode::Materialized,
            ..Default::default()
        },
    )
    .tree;
    validate_plan(&t);
    assert!(matches!(&t,JoinTree::HashJoin{vars,..}if vars==&vec![0,1]));
    assert_eq!(eval(&g, &t), truth(&g, &q));
    let star = [p(v(0), c(10), v(1)), p(v(0), c(11), v(2))];
    let t = plan_bgp_with_options(
        &star,
        &s,
        PlannerOptions {
            binary_mode: BinaryJoinMode::Materialized,
            ..Default::default()
        },
    )
    .tree;
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
                    ..Default::default()
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
    stats.cache_pattern_counts(&q, &g).unwrap();
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
    stats.cache_pattern_counts(&q, &g).unwrap();
    assert_eq!(stats.estimate_bgp(&q), 6.0);
    assert_eq!(truth(&g, &q).len(), 6);
}

#[test]
fn index_nested_loop_model_matches_runtime_executor_bindings() {
    use triplelight::exec::{Options, QueryResult, query_with};
    use triplelight::storage::MemorySnapshot;
    let snapshot=MemorySnapshot::from_ntriples(
        "<urn:a> <urn:p> <urn:b> .\n<urn:a> <urn:p> <urn:c> .\n<urn:b> <urn:q> <urn:a> .\n<urn:c> <urn:q> <urn:a> .\n<urn:a> <urn:r> <urn:a> .".as_bytes()
    ).unwrap();
    let stats = Stats::from_snapshot(&snapshot).unwrap();
    fn normalized(result: QueryResult) -> Vec<Vec<Option<String>>> {
        let QueryResult::Solutions { rows, .. } = result else {
            panic!("SELECT expected")
        };
        let mut rows: Vec<_> = rows
            .into_iter()
            .map(|r| r.into_iter().map(|t| t.map(|t| t.to_string())).collect())
            .collect();
        rows.sort();
        rows
    }
    for text in [
        "SELECT * WHERE { ?x <urn:p> ?y . ?y <urn:q> ?x }",
        "SELECT * WHERE { ?x <urn:p> ?y . ?y <urn:q> ?z . ?z <urn:r> ?x }",
        "SELECT * WHERE { ?x <urn:p> ?y . ?x <urn:p> ?y }",
        "SELECT * WHERE { ?x <urn:r> ?x . ?x <urn:p> ?y }",
        "SELECT * WHERE { ?x <urn:missing> ?y . ?x <urn:p> ?z }",
        "SELECT * WHERE {}",
    ] {
        let greedy = normalized(
            query_with(
                &snapshot,
                text,
                &Options {
                    stats: None,
                    wcoj: false,
                    raw_ids: false,
                    general_group: false,
                    count_pushdown: true,
                    range_pushdown: true,
                    char_pairs: true,
                    sorted_probe: true,
                    force_wide_tables: false, auto_wide_tables: false,
                    csr_paths: true,
                    entailment: Default::default(),
                    interrupt: None,
                    memory_budget: None,
                },
            )
            .unwrap(),
        );
        let planned = normalized(
            query_with(
                &snapshot,
                text,
                &Options {
                    stats: Some(&stats),
                    wcoj: false,
                    raw_ids: false,
                    general_group: false,
                    count_pushdown: true,
                    range_pushdown: true,
                    char_pairs: true,
                    sorted_probe: true,
                    force_wide_tables: false, auto_wide_tables: false,
                    csr_paths: true,
                    entailment: Default::default(),
                    interrupt: None,
                    memory_budget: None,
                },
            )
            .unwrap(),
        );
        assert_eq!(greedy, planned, "{text}");
        for mode in [None, Some(&stats)] {
            let trie = normalized(
                query_with(
                    &snapshot,
                    text,
                    &Options {
                        stats: mode,
                        wcoj: true,
                        raw_ids: false,
                        general_group: false,
                        count_pushdown: true,
                        range_pushdown: true,
                        char_pairs: true,
                        sorted_probe: true,
                        force_wide_tables: false, auto_wide_tables: false,
                        csr_paths: true,
                        entailment: Default::default(),
                        interrupt: None,
                        memory_budget: None,
                    },
                )
                .unwrap(),
            );
            assert_eq!(greedy, trie, "trie order result mismatch: {text}");
        }
    }
}

#[derive(Clone, Copy)]
enum ReadFault {
    Lookup,
    FullScan,
    Count,
    ObjectScan,
    SubjectScan,
}
struct Faulty<'a> {
    inner: &'a dyn Snapshot,
    fault: ReadFault,
}
impl Dictionary for Faulty<'_> {
    fn lookup(&self, t: &Term) -> std::io::Result<Option<TermId>> {
        if matches!(self.fault, ReadFault::Lookup) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "planted dictionary read failure",
            ));
        }
        self.inner.lookup(t)
    }
    fn resolve(&self, id: TermId) -> std::io::Result<Term> {
        self.inner.resolve(id)
    }
    fn with_term(&self, id: TermId, f: &mut dyn FnMut(triplelight_rdf::TermRef<'_>)) -> std::io::Result<()> {
        self.inner.with_term(id, f)
    }
}
impl Snapshot for Faulty<'_> {
    fn scan<'a>(
        &'a self,
        perm: Perm,
        prefix: &[TermId],
    ) -> Box<dyn Iterator<Item = Result<TripleBatch, std::io::Error>> + 'a> {
        let fail = match self.fault {
            ReadFault::FullScan => prefix.is_empty(),
            ReadFault::ObjectScan => perm == Perm::Pos,
            ReadFault::SubjectScan => perm == Perm::Spo && !prefix.is_empty(),
            ReadFault::Count | ReadFault::Lookup => false,
        };
        if fail {
            // A successful batch followed by an error must still abort the read.
            Box::new(
                std::iter::once(Ok(TripleBatch::default())).chain(std::iter::once(Err(
                    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "planted read failure"),
                ))),
            )
        } else {
            self.inner.scan(perm, prefix)
        }
    }
    fn count(&self, perm: Perm, prefix: &[TermId]) -> Result<u64, std::io::Error> {
        if matches!(self.fault, ReadFault::Count) {
            Err(std::io::Error::other("planted count failure"))
        } else {
            self.inner.count(perm, prefix)
        }
    }
    fn seek(
        &self,
        perm: Perm,
        prefix: &[TermId],
        target: TermId,
    ) -> Result<Option<TermId>, std::io::Error> {
        self.inner.seek(perm, prefix, target)
    }
}
#[test]
fn storage_failures_abort_stats_and_atomic_refinement() {
    use std::error::Error;
    let g = Graph::new((1..=130).map(|i| [i, 10, 1000 + i]).collect());
    let error = Stats::from_snapshot(&Faulty {
        inner: &g,
        fault: ReadFault::FullScan,
    })
    .unwrap_err();
    assert!(matches!(error, StatsError::Storage(_)));
    assert!(
        error
            .source()
            .unwrap()
            .to_string()
            .contains("planted read failure")
    );
    let qs = [p(v(0), c(10), v(1)), p(v(0), c(10), c(1001))];
    for fault in [
        ReadFault::Count,
        ReadFault::ObjectScan,
        ReadFault::SubjectScan,
    ] {
        let mut stats = g.stats();
        let before = format!("{stats:?}");
        let e = stats
            .cache_pattern_counts(&qs, &Faulty { inner: &g, fault })
            .unwrap_err();
        assert!(matches!(e, StatsError::Storage(_)));
        assert_eq!(
            format!("{stats:?}"),
            before,
            "partial cache must not survive failure"
        );
        stats.cache_pattern_counts(&qs, &g).unwrap();
        assert!(stats.estimate_pattern(&qs[0]).exact);
    }
}
#[test]
fn optimizer_count_failure_reaches_query_error() {
    use triplelight::exec::{Options, query_with};
    use triplelight::storage::MemorySnapshot;
    let g = MemorySnapshot::from_ntriples(
        "<urn:a> <urn:p> <urn:b> .\n<urn:b> <urn:q> <urn:c> .".as_bytes(),
    )
    .unwrap();
    let stats = Stats::from_snapshot(&g).unwrap();
    let faulty = Faulty {
        inner: &g,
        fault: ReadFault::Count,
    };
    let e = query_with(
        &faulty,
        "SELECT * WHERE { ?x <urn:p> ?y . ?y <urn:q> ?z }",
        &Options {
            stats: Some(&stats),
            wcoj: false,
            raw_ids: false,
            general_group: false,
            count_pushdown: true,
            range_pushdown: true,
            char_pairs: true,
            sorted_probe: true,
            force_wide_tables: false, auto_wide_tables: false,
            csr_paths: true,
            entailment: Default::default(),
            interrupt: None,
            memory_budget: None,
        },
    )
    .unwrap_err();
    assert_eq!(e.kind, triplelight::exec::QueryErrorKind::Storage);
    assert!(e.to_string().contains("planted count failure"));
}

#[test]
fn shared_synopses_keep_query_caches_isolated() {
    let g = Graph::new((1..=130).map(|i| [i, 10, 1000 + i]).collect());
    let original = g.stats();
    let before = format!("{original:?}");
    // That the clone shares the synopsis itself is `stats::persist_tests::clone_shares_the_synopsis`.
    let mut query_stats = original.clone();
    query_stats
        .cache_pattern_counts(&[p(v(0), c(10), c(1001))], &g)
        .unwrap();
    assert_eq!(before, format!("{original:?}"));
    assert_eq!(
        query_stats.estimate_pattern(&p(v(0), c(10), c(1001))).rows,
        1.0
    );
}

#[test]
fn variable_order_search_matches_exhaustive_cost_and_rejects_invalid_orders() {
    use triplelight::opt::planner::{choose_variable_order, variable_order_work};
    let g = Graph::new(
        (1..=50)
            .flat_map(|i| [[i, 10, i % 7], [i % 7, 11, i % 3], [i % 3, 12, i]])
            .collect(),
    );
    let stats = g.stats();
    let patterns = [
        p(v(0), c(10), v(1)),
        p(v(1), c(11), v(2)),
        p(v(2), c(12), v(0)),
    ];
    let order = choose_variable_order(&patterns, &stats);
    let cost = variable_order_work(&patterns, &stats, &order).unwrap();
    let mut best = f64::INFINITY;
    for a in 0..3 {
        for b in 0..3 {
            for c in 0..3 {
                if let Some(work) = variable_order_work(&patterns, &stats, &[a, b, c]) {
                    best = best.min(work);
                }
            }
        }
    }
    assert_eq!(cost, best);
    for bad in [&[0, 0, 2][..], &[0, 1][..], &[0, 1, 99][..]] {
        assert!(variable_order_work(&patterns, &stats, bad).is_none());
    }
    // More than 12 variables use bounded greedy search, preserving the full permutation.
    let chain: Vec<_> = (0..15).map(|i| p(v(i), c(10), v(i + 1))).collect();
    let large = choose_variable_order(&chain, &stats);
    assert_eq!(large.len(), 16);
    assert!(variable_order_work(&chain, &stats, &large).is_some());
    assert_eq!(large, choose_variable_order(&chain, &stats));
}

#[test]
fn work_gate_keeps_small_queries_greedy_and_admits_expanding_stars() {
    use triplelight::opt::planner::{MIN_OPTIMIZER_WORK, estimated_greedy_work};
    let small = Graph::new(vec![[1, 10, 2], [2, 11, 3]]);
    let q = [p(v(0), c(10), v(1)), p(v(1), c(11), v(2))];
    assert!(estimated_greedy_work(&q, &small.stats()) < MIN_OPTIMIZER_WORK);
    let mut triples = Vec::new();
    for s in 1..=100 {
        for o in 1000..1050 {
            for pred in 10..13 {
                triples.push([s, pred, o]);
            }
        }
    }
    let large = Graph::new(triples);
    let star = [
        p(v(0), c(10), v(1)),
        p(v(0), c(11), v(2)),
        p(v(0), c(12), v(3)),
    ];
    assert!(estimated_greedy_work(&star, &large.stats()) >= MIN_OPTIMIZER_WORK);
}

#[test]
fn wide_high_work_bgp_preserves_original_runtime_results() {
    use triplelight::exec::{Options, QueryResult, query_with};
    use triplelight::opt::planner::estimated_greedy_work;
    use triplelight::storage::MemorySnapshot;
    let mut input = String::new();
    for s in 0..500 {
        for o in 0..50 {
            input.push_str(&format!("<urn:s{s}> <urn:p> <urn:o{o}> .\n"));
        }
    }
    let graph = MemorySnapshot::from_ntriples(input.as_bytes()).unwrap();
    let stats = Stats::from_snapshot(&graph).unwrap();
    let pred = graph
        .lookup(&triplelight_rdf::NamedNode::new("urn:p").unwrap().into())
        .unwrap()
        .unwrap();
    let patterns = vec![p(v(0), Slot::Const(pred), v(1)); 8];
    assert!(estimated_greedy_work(&patterns, &stats) >= 100_000.0);
    let query = format!("SELECT ?s ?o WHERE {{ {} }}", "?s <urn:p> ?o . ".repeat(8));
    let normalize = |r| {
        let QueryResult::Solutions { rows, .. } = r else {
            panic!("SELECT expected")
        };
        let mut rows: Vec<_> = rows
            .into_iter()
            .map(|r| {
                r.into_iter()
                    .map(|t| t.map(|t| t.to_string()))
                    .collect::<Vec<_>>()
            })
            .collect();
        rows.sort();
        rows
    };
    let expected = normalize(
        query_with(
            &graph,
            &query,
            &Options {
                stats: None,
                wcoj: true,
                raw_ids: false,
                general_group: false,
                count_pushdown: true,
                range_pushdown: true,
                char_pairs: true,
                sorted_probe: true,
                force_wide_tables: false, auto_wide_tables: false,
                csr_paths: true,
                entailment: Default::default(),
                interrupt: None,
                memory_budget: None,
            },
        )
        .unwrap(),
    );
    let actual = normalize(
        query_with(
            &graph,
            &query,
            &Options {
                stats: Some(&stats),
                wcoj: true,
                raw_ids: false,
                general_group: false,
                count_pushdown: true,
                range_pushdown: true,
                char_pairs: true,
                sorted_probe: true,
                force_wide_tables: false, auto_wide_tables: false,
                csr_paths: true,
                entailment: Default::default(),
                interrupt: None,
                memory_budget: None,
            },
        )
        .unwrap(),
    );
    assert_eq!(actual.len(), 25000);
    assert_eq!(actual, expected);
}

#[test]
fn optimizer_dictionary_lookup_failure_is_not_an_empty_result() {
    use triplelight::exec::{Options, QueryErrorKind, query_with};
    use triplelight::storage::MemorySnapshot;
    let g = MemorySnapshot::from_ntriples("<urn:a> <urn:p> <urn:b> .".as_bytes()).unwrap();
    let stats = Stats::from_snapshot(&g).unwrap();
    let faulty = Faulty {
        inner: &g,
        fault: ReadFault::Lookup,
    };
    let error = query_with(
        &faulty,
        "SELECT * WHERE { ?s <urn:p> ?o }",
        &Options {
            stats: Some(&stats),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.kind, QueryErrorKind::Storage);
    assert!(
        error
            .to_string()
            .contains("planted dictionary read failure")
    );
}

// ---------- characteristic pairs: a BSBM-like graph where `country` is shared by producers and reviewers ----------

const TYPE: u64 = 1;
const PRODUCER: u64 = 2;
const COUNTRY: u64 = 3;
const LABEL: u64 = 4;
const NAME: u64 = 5;
const REVIEW_FOR: u64 = 6;
const REVIEWER: u64 = 7;
const PRODUCT: u64 = 10;
const AT: u64 = 11;
const DE: u64 = 12;
const US: u64 = 13;
const GB: u64 = 14;

/// 2000 products {type, producer} of 100 producers {country, label} (5 in AT); 1000 reviewers {country, name}: 400 in
/// AT, 100 in US, 500 in GB; 10000 reviews {reviewFor, reviewer}. `?producer country AT` has 405 rows, 400 of them
/// reviewers, which only the characteristic pairs of `producer` can rule out.
fn bsbm_like() -> Graph {
    let mut t = Vec::new();
    for i in 0..2000 {
        t.push([10_000 + i, TYPE, PRODUCT]);
        t.push([10_000 + i, PRODUCER, 20_000 + i % 100]);
    }
    for j in 0..100 {
        t.push([20_000 + j, COUNTRY, if j < 5 { AT } else { DE }]);
        t.push([20_000 + j, LABEL, 100_000 + j]);
    }
    for r in 0..1000 {
        t.push([30_000 + r, COUNTRY, if r < 400 { AT } else if r < 500 { US } else { GB }]);
        t.push([30_000 + r, NAME, 200_000 + r]);
    }
    for k in 0..10_000 {
        t.push([40_000 + k, REVIEW_FOR, 10_000 + k % 2000]);
        t.push([40_000 + k, REVIEWER, 30_000 + (k * 7) % 1000]);
    }
    Graph::new(t)
}
fn leaf_order(t: &JoinTree, out: &mut Vec<TriplePattern>) {
    match t {
        JoinTree::Scan { pattern, .. } => out.push(*pattern),
        JoinTree::IndexNestedLoop { outer, pattern, .. } => {
            leaf_order(outer, out);
            out.push(*pattern);
        }
        other => panic!("index nested-loop plan expected, got {other:?}"),
    }
}

#[test]
fn characteristic_pairs_start_from_the_selective_producer_country() {
    let g = bsbm_like();
    // ?product=0 ?producer=1 ?review=2 ?reviewer=3, in the query's textual order.
    let product_producer = p(v(0), c(PRODUCER), v(1));
    let producer_at = p(v(1), c(COUNTRY), c(AT));
    let review_for = p(v(2), c(REVIEW_FOR), v(0));
    let review_reviewer = p(v(2), c(REVIEWER), v(3));
    let reviewer_us = p(v(3), c(COUNTRY), c(US));
    let q = [product_producer, producer_at, review_for, review_reviewer, reviewer_us];
    // As at runtime: exact per-pattern counts first.
    let mut on = g.stats();
    on.cache_pattern_counts(&q, &g).unwrap();
    assert!(on.uses_char_pairs());
    let mut off = on.clone();
    off.set_char_pairs(false);
    assert!(!off.uses_char_pairs());

    // The estimate the planner gets wrong without pairs: AT producers' products.
    let join = [product_producer, producer_at];
    assert_eq!(truth(&g, &join).len(), 100);
    let with_pairs = on.estimate_bgp(&join);
    assert!((with_pairs - 100.0).abs() < 1e-6, "{with_pairs}");
    assert!(off.estimate_bgp(&join) >= 1000.0, "{}", off.estimate_bgp(&join));

    let order = |stats: &Stats| {
        let d = plan_bgp_with_options(&q, stats, PlannerOptions::default());
        validate_plan(&d.tree);
        let mut leaves = Vec::new();
        leaf_order(&d.tree, &mut leaves);
        let mut a = eval(&g, &d.tree);
        let mut b = truth(&g, &q);
        a.sort();
        b.sort();
        assert_eq!(a, b);
        leaves
    };
    // Today's order: the reviewer country first.
    assert_eq!(order(&off), vec![reviewer_us, review_reviewer, review_for, product_producer, producer_at]);
    // With pairs: producers in AT, their products, the reviews, then the US filter.
    assert_eq!(order(&on), vec![producer_at, product_producer, review_for, review_reviewer, reviewer_us]);
}

#[test]
fn characteristic_pairs_do_not_change_answers() {
    use std::fmt::Write;
    use triplelight::exec::{Options, QueryResult, query_with};
    use triplelight::storage::MemorySnapshot;
    // The same shape as `bsbm_like`, larger so the runtime planner gate (`MIN_OPTIMIZER_WORK`) is passed.
    let mut nt = String::new();
    let rdf_type = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    for t in 0..10 {
        writeln!(nt, "<urn:type/{t}> {rdf_type} <urn:ProductType> .").unwrap();
    }
    for i in 0..4000 {
        writeln!(nt, "<urn:product/{i}> {rdf_type} <urn:type/{}> .", i % 10).unwrap();
        writeln!(nt, "<urn:product/{i}> <urn:producer> <urn:producer/{}> .", i % 100).unwrap();
    }
    for j in 0..100 {
        writeln!(nt, "<urn:producer/{j}> <urn:country> <urn:{}> .", if j < 5 { "AT" } else { "DE" }).unwrap();
        writeln!(nt, "<urn:producer/{j}> <urn:label> \"producer {j}\" .").unwrap();
    }
    for r in 0..2000 {
        let country = if r < 800 { "AT" } else if r < 1000 { "US" } else { "GB" };
        writeln!(nt, "<urn:reviewer/{r}> <urn:country> <urn:{country}> .").unwrap();
        writeln!(nt, "<urn:reviewer/{r}> <urn:name> \"reviewer {r}\" .").unwrap();
    }
    for k in 0..40_000 {
        writeln!(nt, "<urn:review/{k}> <urn:reviewFor> <urn:product/{}> .", k % 4000).unwrap();
        writeln!(nt, "<urn:review/{k}> <urn:reviewer> <urn:reviewer/{}> .", (k * 7) % 2000).unwrap();
    }
    let snapshot = MemorySnapshot::from_ntriples(nt.as_bytes()).unwrap();
    let stats = Stats::from_snapshot(&snapshot).unwrap();
    assert!(stats.uses_char_pairs());
    fn normalized(result: QueryResult) -> Vec<Vec<Option<String>>> {
        let QueryResult::Solutions { rows, .. } = result else {
            panic!("SELECT expected")
        };
        let mut rows: Vec<_> = rows
            .into_iter()
            .map(|r| r.into_iter().map(|t| t.map(|t| t.to_string())).collect())
            .collect();
        rows.sort();
        rows
    }
    for text in [
        "SELECT * WHERE { ?product <urn:producer> ?producer . ?producer <urn:country> <urn:AT> . ?review <urn:reviewFor> ?product . ?review <urn:reviewer> ?reviewer . ?reviewer <urn:country> <urn:US> }",
        "SELECT * WHERE { ?productType a <urn:ProductType> . ?product a ?productType . ?product <urn:producer> ?producer . ?producer <urn:country> <urn:AT> . ?review <urn:reviewFor> ?product . ?review <urn:reviewer> ?reviewer . ?reviewer <urn:country> <urn:US> }",
        "SELECT * WHERE { ?review <urn:reviewFor> ?product . ?product <urn:producer> ?producer . ?producer <urn:country> ?country }",
        "SELECT * WHERE { ?review <urn:reviewer> ?reviewer . ?reviewer <urn:country> ?c . ?review <urn:reviewFor> ?product . ?product <urn:producer> ?producer . ?producer <urn:country> ?c }",
        "SELECT ?c (COUNT(*) AS ?n) WHERE { ?review <urn:reviewFor> ?product . ?product <urn:producer> ?producer . ?producer <urn:country> ?c . ?review <urn:reviewer> ?reviewer . ?reviewer <urn:country> <urn:US> } GROUP BY ?c",
    ] {
        let run = |stats: Option<&Stats>, char_pairs: bool| {
            normalized(query_with(&snapshot, text, &Options { stats, char_pairs, ..Default::default() }).unwrap())
        };
        let reference = run(None, true);
        assert!(!reference.is_empty(), "{text}");
        assert_eq!(run(Some(&stats), true), reference, "{text}");
        assert_eq!(run(Some(&stats), false), reference, "{text}");
    }
}
