//! SPO-stream statistics. Characteristic-set predicate presence is exact;
//! multiplicity independence, bounded object frequencies and chain NDVs are estimates.
use super::plan::{Slot, TriplePattern};
use crate::model::{Perm, TermId};
use crate::model::TripleBatch;
use crate::storage::{Snapshot, StorageError};

/// A fresh SPO scan of the triples statistics are built from.
pub type StatsScan<'a> = Box<dyn Iterator<Item = Result<TripleBatch, StorageError>> + 'a>;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::sync::Arc;

const HISTOGRAM_SIZE: usize = 64;
const MAX_CARD: f64 = 1e300;
const MAX_CS_REFINEMENT_ROWS: u64 = 4096;
/// Characteristic pairs kept per predicate (the rest is summed into `PredicatePairs::other`).
pub const PAIRS_PER_PREDICATE: usize = 64;
/// Distinct (predicate, subject CS, object CS) keys counted exactly while building; triples of keys first seen after
/// this many go straight to their predicate's `other` bucket. Bounds the build's memory on CS-rich data.
const MAX_PAIR_KEYS: usize = 1 << 20;

#[derive(Clone, Debug)]
struct Distinct {
    registers: [u8; 128],
}
impl Default for Distinct {
    fn default() -> Self {
        Self {
            registers: [0; 128],
        }
    }
}
impl Distinct {
    fn add(&mut self, id: TermId) {
        // SplitMix64 finalizer: tagged/sequential IDs must not become skewed registers.
        let mut x = id.0.wrapping_add(0x9e3779b97f4a7c15);
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
        x ^= x >> 31;
        let i = (x & 127) as usize;
        let rank = ((x >> 7).leading_zeros() - 7 + 1).min(58) as u8;
        self.registers[i] = self.registers[i].max(rank);
    }
    fn count(&self) -> f64 {
        let m = 128.0;
        let z: f64 = self
            .registers
            .iter()
            .map(|r| 2f64.powi(-i32::from(*r)))
            .sum();
        let raw = (0.7213 / (1.0 + 1.079 / m)) * m * m / z;
        let zeros = self.registers.iter().filter(|r| **r == 0).count();
        if raw <= 2.5 * m && zeros > 0 {
            m * (m / zeros as f64).ln()
        } else {
            raw
        }
    }
}

/// Misra-Gries counters: exact until overflow; absent values after overflow are NOT zero.
#[derive(Clone, Debug, Default)]
struct Frequencies {
    counts: BTreeMap<TermId, u64>,
    rounds: u64,
}
impl Frequencies {
    fn add(&mut self, id: TermId) {
        if let Some(c) = self.counts.get_mut(&id) {
            *c += 1;
        } else if self.counts.len() < HISTOGRAM_SIZE {
            self.counts.insert(id, 1);
        } else {
            self.rounds += 1;
            self.counts.retain(|_, c| {
                *c -= 1;
                *c > 0
            });
        }
    }
    fn estimate(&self, id: TermId) -> f64 {
        self.counts.get(&id).copied().unwrap_or(0) as f64 + self.rounds as f64 / 2.0
    }
}

#[derive(Clone, Debug, Default)]
struct Property {
    triples: u64,
    subjects: u64,
    objects: Distinct,
    frequencies: Frequencies,
    equality: [u64; 8],
}
impl Property {
    fn add(&mut self, t: [TermId; 3]) {
        self.triples += 1;
        self.objects.add(t[2]);
        self.frequencies.add(t[2]);
        self.equality[equality_mask(t)] += 1;
    }
    fn ndv_objects(&self) -> f64 {
        self.objects
            .count()
            .max(1.0)
            .min(self.triples.max(1) as f64)
    }
}

/// Subjects with exactly this set of outgoing predicates; no subject-ID table is retained.
#[derive(Clone, Debug)]
pub struct CharacteristicSet {
    pub predicates: Vec<TermId>,
    pub subjects: u64,
    properties: BTreeMap<TermId, Property>,
}

/// Characteristic pairs (Gubichev and Neumann, EDBT 2014) of one predicate `p`: the triples `(s p o)` whose object
/// is itself a subject, counted by (characteristic set of `s`, characteristic set of `o`). Literals and IRIs that are
/// never subjects have no set and are not counted. At most `PAIRS_PER_PREDICATE` pairs are kept (the largest);
/// the triples of the others are summed in `other`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PredicatePairs {
    /// (subject set index, object set index, triples), sorted by (subject set, object set); every count is positive.
    pub pairs: Vec<(u32, u32, u64)>,
    /// Linked triples of this predicate whose pair is not in `pairs`.
    pub other: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub triple_count: u64,
    pub subject_count: u64,
    pub duplicate_triples_ignored: u64,
    characteristic_sets: Arc<Vec<CharacteristicSet>>,
    cs_lookup: Arc<HashMap<Vec<TermId>, usize>>,
    predicates: Arc<BTreeMap<TermId, Property>>,
    objects: Distinct,
    equality: [u64; 8],
    exact_counts: HashMap<TriplePattern, u64>,
    exact_object_cs_counts: HashMap<TriplePattern, Vec<u64>>,
    star_estimates: HashMap<Vec<TriplePattern>, f64>,
    planning_domains: HashMap<TriplePattern, BTreeMap<u32, f64>>,
    /// Characteristic pairs by predicate (empty for statistics loaded from a `TLSTATS1` payload).
    char_pairs: Arc<BTreeMap<TermId, PredicatePairs>>,
    /// Ablation switch: when set, `estimate_bgp` ignores `char_pairs` (the pre-pairs estimates, bit for bit).
    char_pairs_off: bool,
}

#[derive(Debug)]
pub enum StatsError {
    Storage(crate::storage::StorageError),
    UnsortedSpo,
    UnequalBatchColumns,
}
impl fmt::Display for StatsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for StatsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Storage(e) => Some(e),
            _ => None,
        }
    }
}
impl From<crate::storage::StorageError> for StatsError {
    fn from(e: crate::storage::StorageError) -> Self {
        Self::Storage(e)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Estimate {
    pub rows: f64,
    pub exact: bool,
}

impl Stats {
    /// Input must be lexicographically sorted SPO. Adjacent duplicate triples are ignored.
    /// The input is buffered: characteristic pairs need a second pass (see `from_snapshot`).
    pub fn from_spo(triples: impl IntoIterator<Item = [TermId; 3]>) -> Result<Self, StatsError> {
        let triples: Vec<_> = triples.into_iter().collect();
        let mut builder = Builder::default();
        for t in &triples {
            builder.push(*t)?;
        }
        let (mut stats, mut pairs) = builder.finish();
        for t in triples {
            pairs.push(t);
        }
        stats.char_pairs = Arc::new(pairs.finish());
        Ok(stats)
    }
    /// Two SPO passes. The first builds everything but the characteristic pairs, keeping each subject's set index
    /// (12 bytes per subject, the pairs' only large auxiliary memory); the second counts the pairs, looking the
    /// object's set up by binary search. Auxiliary memory is otherwise bounded object sketches + CS/property
    /// summaries + the largest subject group + at most `MAX_PAIR_KEYS` pair counters.
    /// Bump `STATS_ALGO_VERSION` when this changes what it computes (persisted statistics are bound to it).
    pub fn from_snapshot(snapshot: &dyn Snapshot) -> Result<Self, StatsError> {
        Self::from_scans(|| snapshot.scan_for_stats())
    }
    /// The same two passes over any source that can start a fresh SPO scan each time it is asked (a snapshot's
    /// `scan_for_stats`, or an overlay's base alone, `Overlay::scan_base_for_stats`).
    pub fn from_scans<'a>(scan: impl Fn() -> StatsScan<'a>) -> Result<Self, StatsError> {
        fn each<'a>(
            scan: &impl Fn() -> StatsScan<'a>,
            mut f: impl FnMut([TermId; 3]) -> Result<(), StatsError>,
        ) -> Result<(), StatsError> {
            for batch in scan() {
                let batch = batch?;
                let n = batch.cols[0].len();
                if batch.cols[1].len() != n || batch.cols[2].len() != n {
                    return Err(StatsError::UnequalBatchColumns);
                }
                for i in 0..n {
                    f([batch.cols[0][i], batch.cols[1][i], batch.cols[2][i]])?;
                }
            }
            Ok(())
        }
        let mut builder = Builder::default();
        each(&scan, |t| builder.push(t))?;
        let (mut stats, mut pairs) = builder.finish();
        each(&scan, |t| {
            pairs.push(t);
            Ok(())
        })?;
        stats.char_pairs = Arc::new(pairs.finish());
        Ok(stats)
    }
    /// Number of characteristic sets (distinct predicate sets of the subjects) in the synopsis.
    pub fn characteristic_set_count(&self) -> usize {
        self.characteristic_sets.len()
    }
    /// Use (`true`, the default) or ignore the characteristic pairs in `estimate_bgp` (an ablation switch; with
    /// `false`, or with no pairs, every estimate is the pre-pairs one).
    pub fn set_char_pairs(&mut self, on: bool) {
        self.char_pairs_off = !on;
    }
    /// Whether `estimate_bgp` uses characteristic pairs: switched on and present.
    pub fn uses_char_pairs(&self) -> bool {
        !self.char_pairs_off && !self.char_pairs.is_empty()
    }
    /// The characteristic pairs of predicate `p`, if any of its objects is a subject.
    pub fn char_pairs(&self, p: TermId) -> Option<&PredicatePairs> {
        self.char_pairs.get(&p)
    }
    /// Fill query-specific exact prefix counts without retaining the snapshot or copying triples.
    /// Repeated-variable patterns need equality filtering; their estimates remain synopsis-based.
    pub fn cache_pattern_counts(
        &mut self,
        patterns: &[TriplePattern],
        snapshot: &dyn Snapshot,
    ) -> Result<(), StatsError> {
        let mut exact_counts = self.exact_counts.clone();
        let mut exact_object_cs_counts = self.exact_object_cs_counts.clone();
        for pattern in patterns {
            if has_repeated_variable(pattern) {
                continue;
            }
            let perm = scan_permutation(pattern, None);
            let slots = slots(pattern);
            let prefix: Vec<_> = perm
                .order()
                .iter()
                .take_while(|i| matches!(slots[**i], Slot::Const(_)))
                .map(|i| match slots[*i] {
                    Slot::Const(id) => id,
                    _ => unreachable!(),
                })
                .collect();
            let count = snapshot.count(perm, &prefix)?;
            exact_counts.insert(*pattern, count);
            // A small POS range can repair a bounded histogram's rare-object error without
            // executing a join. Large ranges retain the synopsis to cap planning overhead.
            if let (Slot::Var(_), Slot::Const(pred), Slot::Const(object)) =
                (pattern.s, pattern.p, pattern.o)
                && count <= MAX_CS_REFINEMENT_ROWS
                && self
                    .predicates
                    .get(&pred)
                    .is_some_and(|p| p.frequencies.rounds > 0)
            {
                let mut counts = vec![0; self.characteristic_sets.len()];
                for batch in snapshot.scan(Perm::Pos, &[pred, object]) {
                    let batch = batch?;
                    if batch.cols.iter().any(|c| c.len() != batch.cols[0].len()) {
                        return Err(StatsError::UnequalBatchColumns);
                    }
                    for subject in &batch.cols[2] {
                        let mut predicates = BTreeSet::new();
                        for b in snapshot.scan(Perm::Spo, &[*subject]) {
                            let b = b?;
                            if b.cols.iter().any(|c| c.len() != b.cols[0].len()) {
                                return Err(StatsError::UnequalBatchColumns);
                            }
                            predicates.extend(b.cols[1].iter().copied());
                        }
                        let key: Vec<_> = predicates.into_iter().collect();
                        if let Some(&index) = self.cs_lookup.get(&key) {
                            counts[index] += 1;
                        }
                    }
                }
                exact_object_cs_counts.insert(*pattern, counts);
            }
        }
        self.exact_counts = exact_counts;
        self.exact_object_cs_counts = exact_object_cs_counts;
        self.star_estimates.clear();
        self.planning_domains.clear();
        Ok(())
    }
    /// Replace the exact row count of `pattern` with `rows`, the rows the executor will actually scan for it (e.g. an
    /// object ID range pushed down from a FILTER). Call after `cache_pattern_counts`; star estimates keep using the
    /// synopsis, so this only changes the pattern's own scan estimate.
    pub fn set_exact_count(&mut self, pattern: TriplePattern, rows: u64) {
        self.exact_counts.insert(pattern, rows);
        self.exact_object_cs_counts.remove(&pattern);
        self.star_estimates.clear();
        self.planning_domains.clear();
    }
    pub fn estimate_pattern(&self, pattern: &TriplePattern) -> Estimate {
        if let Some(n) = self.exact_counts.get(pattern) {
            return Estimate {
                rows: *n as f64,
                exact: true,
            };
        }
        let property = if let Slot::Const(p) = pattern.p {
            match self.predicates.get(&p) {
                Some(x) => Some(x),
                None => {
                    return Estimate {
                        rows: 0.0,
                        exact: true,
                    };
                }
            }
        } else {
            None
        };
        let n = property.map_or(self.triple_count, |p| p.triples);
        if n == 0 {
            return Estimate {
                rows: 0.0,
                exact: true,
            };
        }
        let mut rows = n as f64;
        let mut exact =
            !matches!(pattern.s, Slot::Const(_)) && !matches!(pattern.o, Slot::Const(_));
        if let Slot::Const(o) = pattern.o {
            if let Some(p) = property {
                rows = p.frequencies.estimate(o).min(rows);
                exact = p.frequencies.rounds == 0 && !matches!(pattern.s, Slot::Const(_));
            } else {
                rows /= self.objects.count().max(1.0);
                exact = false;
            }
        }
        if matches!(pattern.s, Slot::Const(_)) {
            rows /= property.map_or(self.subject_count, |p| p.subjects).max(1) as f64;
            exact = false;
        }
        let mask = variable_equality_mask(pattern);
        if mask != 0 {
            let eq = property.map_or(&self.equality, |p| &p.equality);
            let equal_rows: u64 = eq
                .iter()
                .enumerate()
                .filter(|(i, _)| i & mask == mask)
                .map(|(_, c)| *c)
                .sum();
            rows *= equal_rows as f64 / n as f64;
            // With no S/O constants the mask count itself is exact.
            exact = !matches!(pattern.s, Slot::Const(_)) && !matches!(pattern.o, Slot::Const(_));
        }
        Estimate {
            rows: cardinal(rows),
            exact,
        }
    }
    /// Characteristic-set stars, then uniform NDV selectivity between remaining groups/chains.
    /// Estimates guide plans; approximate zero is never used as a correctness filter.
    pub fn estimate_bgp(&self, patterns: &[TriplePattern]) -> f64 {
        if patterns.is_empty() {
            return 1.0;
        }
        let mut unique = Vec::new();
        for p in patterns {
            if !unique.contains(p) {
                unique.push(*p);
            }
        }
        let mut by_subject: BTreeMap<(u8, u64), Vec<TriplePattern>> = BTreeMap::new();
        for p in unique {
            let key = match p.s {
                Slot::Var(v) => (0, u64::from(v)),
                Slot::Const(id) => (1, id.0),
            };
            by_subject.entry(key).or_default().push(p);
        }
        let by_subject: Vec<Vec<TriplePattern>> = by_subject.into_values().collect();
        let mut groups = Vec::new();
        let mut merged = vec![false; by_subject.len()];
        if self.uses_char_pairs() {
            // Stars linked subject-to-object become one group whose rows come from the characteristic pairs; the
            // loop below joins it to the rest exactly as it joins any other group.
            for (members, rows) in self.pair_components(&by_subject) {
                let mut patterns = Vec::new();
                for g in members {
                    merged[g] = true;
                    patterns.extend_from_slice(&by_subject[g]);
                }
                groups.push((patterns, rows));
            }
        }
        for (i, group) in by_subject.into_iter().enumerate() {
            if merged[i] {
                continue;
            }
            if let Some(rows) = self.estimate_star(&group) {
                groups.push((group, rows));
            } else {
                for p in group {
                    groups.push((vec![p], self.estimate_pattern(&p).rows));
                }
            }
        }
        groups.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut seen: BTreeMap<u32, f64> = BTreeMap::new();
        let mut rows = 1.0;
        for (group, count) in groups {
            let domains = self.variable_domains(&group);
            rows = cardinal(rows * count);
            for (var, ndv) in domains {
                let ndv = ndv.min(count.max(1.0)).max(1.0);
                if let Some(prior) = seen.get_mut(&var) {
                    rows /= prior.max(ndv);
                    *prior = prior.min(ndv);
                } else {
                    seen.insert(var, ndv);
                }
            }
            rows = cardinal(rows);
        }
        rows
    }
    fn estimate_star(&self, group: &[TriplePattern]) -> Option<f64> {
        if let Some(rows) = self.star_estimates.get(group) {
            return Some(*rows);
        }
        if group.len() < 2 || group.iter().any(|p| !matches!(p.p, Slot::Const(_))) {
            return None;
        }
        let mut objects = BTreeSet::new();
        for p in group {
            if let Slot::Var(v) = p.o
                && (p.s == p.o || !objects.insert(v))
            {
                return None;
            }
        }
        if matches!(group[0].s, Slot::Const(_))
            && group.iter().all(|p| self.exact_counts.contains_key(p))
        {
            return Some(
                group
                    .iter()
                    .fold(1.0, |n, p| cardinal(n * self.exact_counts[p] as f64)),
            );
        }
        let mut total = 0.0;
        for (cs_index, cs) in self.characteristic_sets.iter().enumerate() {
            total = cardinal(total + self.cs_star_rows(cs_index, cs, group));
        }
        if matches!(group[0].s, Slot::Const(_)) {
            total /= self.subject_count.max(1) as f64;
        }
        Some(cardinal(total))
    }
    /// Rows of the star `group` (constant predicates) over the subjects of set `cs`: the set's subjects times each
    /// pattern's matching triples per subject (multiplicity independence within the set); zero if a predicate is
    /// missing from the set. A constant object uses the set's own object frequencies (or the exact per-set counts
    /// `cache_pattern_counts` gathered).
    fn cs_star_rows(&self, cs_index: usize, cs: &CharacteristicSet, group: &[TriplePattern]) -> f64 {
        let mut rows = cs.subjects as f64;
        for p in group {
            let Slot::Const(pred) = p.p else {
                unreachable!()
            };
            if let Some(prop) = cs.properties.get(&pred) {
                let matching = match p.o {
                    Slot::Const(o) => self.exact_object_cs_counts.get(p).map_or_else(
                        || prop.frequencies.estimate(o),
                        |counts| counts[cs_index] as f64,
                    ),
                    Slot::Var(_) => prop.triples as f64,
                };
                rows = cardinal(rows * matching / cs.subjects as f64);
            } else {
                return 0.0;
            }
        }
        rows
    }
    /// Per characteristic set, the rows of star `group` over that set's subjects. A single pattern with an exact
    /// count is scaled to sum to it (the sets then only say where its rows are).
    fn star_vector(&self, group: &[TriplePattern]) -> Vec<f64> {
        let mut w: Vec<f64> = self
            .characteristic_sets
            .iter()
            .enumerate()
            .map(|(i, cs)| self.cs_star_rows(i, cs, group))
            .collect();
        if group.len() == 1
            && let Some(&n) = self.exact_counts.get(&group[0])
        {
            let sum: f64 = w.iter().sum();
            if sum > 0.0 {
                let scale = n as f64 / sum;
                for x in &mut w {
                    *x = cardinal(*x * scale);
                }
            }
        }
        w
    }
    /// Characteristic-pair estimates (Gubichev and Neumann, EDBT 2014) for the parts of a BGP where stars are joined
    /// subject-to-object. `groups` are the BGP's patterns grouped by subject. A star is a group with a variable
    /// subject and constant predicates, no pattern `?x p ?x` and no object variable repeated inside it. A link is a
    /// pattern `?a p ?b` of star A whose object `?b` is the subject of another star B and occurs nowhere else in the
    /// BGP. Linked stars form components; each component that is a tree, and whose other variables each occur once
    /// in it, is estimated as
    ///
    ///   sum over an assignment of a characteristic set to every star of
    ///     prod over stars S of rows_S(cs_S) * prod over links (A -p-> B) of pairs_p(cs_A, cs_B) / (triples_p(cs_A) * subjects(cs_B))
    ///
    /// where `rows_S(cs)` is `cs_star_rows` (so constant objects are filtered with the per-set object statistics,
    /// which is what tells producers' `country` from reviewers'), `pairs_p` the pair counts, `triples_p(cs)` the
    /// set's `p` triples (the link's own multiplicity in `rows_A`, replaced by the pair count) and `subjects(cs)` the
    /// set's subjects. A sum-product over the tree computes it in one pass per link. The `other` bucket is spread
    /// independently: in proportion to the subject set's `p` triples and the object set's subjects. Sets incompatible
    /// with a star's predicates have `rows_S = 0`, so only compatible pairs count. A component whose estimate is zero
    /// (or not finite), or with a link predicate without pairs, is not returned: its stars keep the independence
    /// estimate.
    fn pair_components(&self, groups: &[Vec<TriplePattern>]) -> Vec<(Vec<usize>, f64)> {
        let star_ok = |g: &[TriplePattern]| {
            let mut objects = BTreeSet::new();
            matches!(g[0].s, Slot::Var(_))
                && g.iter().all(|p| {
                    matches!(p.p, Slot::Const(_))
                        && p.s != p.o
                        && match p.o {
                            Slot::Var(v) => objects.insert(v),
                            Slot::Const(_) => true,
                        }
                })
        };
        let eligible: Vec<bool> = groups.iter().map(|g| star_ok(g.as_slice())).collect();
        let mut star_of: BTreeMap<u32, usize> = BTreeMap::new();
        for (i, g) in groups.iter().enumerate() {
            if let (true, Slot::Var(v)) = (eligible[i], g[0].s) {
                star_of.insert(v, i);
            }
        }
        let mut occurrences: BTreeMap<u32, usize> = BTreeMap::new();
        for p in groups.iter().flatten() {
            for v in slots(p).into_iter().filter_map(|s| if let Slot::Var(v) = s { Some(v) } else { None }) {
                *occurrences.entry(v).or_default() += 1;
            }
        }
        let mut links = Vec::new();
        for (a, g) in groups.iter().enumerate() {
            if !eligible[a] {
                continue;
            }
            for (k, p) in g.iter().enumerate() {
                if let Slot::Var(v) = p.o
                    && let Some(&b) = star_of.get(&v)
                    && b != a
                    && occurrences[&v] == groups[b].len() + 1
                {
                    links.push(Link { from: a, pattern: k, to: b });
                }
            }
        }
        if links.is_empty() {
            return Vec::new();
        }
        // Components of the link graph (union-find over group indices).
        let mut parent: Vec<usize> = (0..groups.len()).collect();
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        }
        for l in &links {
            let (x, y) = (find(&mut parent, l.from), find(&mut parent, l.to));
            if x != y {
                parent[x] = y;
            }
        }
        let mut components: BTreeMap<usize, (Vec<usize>, Vec<Link>)> = BTreeMap::new();
        for l in &links {
            let r = find(&mut parent, l.from);
            components.entry(r).or_default().1.push(*l);
        }
        for g in 0..groups.len() {
            let r = find(&mut parent, g);
            if let Some(c) = components.get_mut(&r) {
                c.0.push(g);
            }
        }
        let mut out = Vec::new();
        for (members, links) in components.into_values() {
            if links.len() + 1 != members.len() {
                continue; // a cycle of links: keep the independence estimate
            }
            // Every variable of the component: a star subject occurs in its star (plus once if linked into);
            // any other variable occurs once. Otherwise a join inside the component would be ignored.
            let mut inside: BTreeMap<u32, usize> = BTreeMap::new();
            for &g in &members {
                for p in &groups[g] {
                    for v in slots(p).into_iter().filter_map(|s| if let Slot::Var(v) = s { Some(v) } else { None }) {
                        *inside.entry(v).or_default() += 1;
                    }
                }
            }
            let consistent = inside.iter().all(|(v, n)| match star_of.get(v) {
                Some(&b) if members.contains(&b) => {
                    *n == groups[b].len() + usize::from(links.iter().any(|l| l.to == b))
                }
                _ => *n == 1,
            });
            if !consistent {
                continue;
            }
            let vectors: BTreeMap<usize, Vec<f64>> =
                members.iter().map(|&g| (g, self.star_vector(&groups[g]))).collect();
            let Some(w) = self.absorb(members[0], None, groups, &links, &vectors) else {
                continue;
            };
            let rows = cardinal(w.iter().sum());
            if rows > 0.0 && rows.is_finite() {
                out.push((members, rows));
            }
        }
        out
    }
    /// The sum-product step of `pair_components`: star `node`'s per-set rows with every subtree hanging off it
    /// (except through `parent_link`) folded in. `None` if a link predicate has no pairs.
    fn absorb(
        &self,
        node: usize,
        parent_link: Option<usize>,
        groups: &[Vec<TriplePattern>],
        links: &[Link],
        vectors: &BTreeMap<usize, Vec<f64>>,
    ) -> Option<Vec<f64>> {
        let sets = &self.characteristic_sets;
        let mut w = vectors[&node].clone();
        for (li, l) in links.iter().enumerate() {
            if Some(li) == parent_link || (l.from != node && l.to != node) {
                continue;
            }
            let Slot::Const(pred) = groups[l.from][l.pattern].p else {
                unreachable!()
            };
            let table = self.char_pairs.get(&pred)?;
            let total_triples = self.predicates.get(&pred)?.triples.max(1) as f64;
            let subject_count = self.subject_count.max(1) as f64;
            let triples_in = |cs: usize| sets[cs].properties.get(&pred).map_or(0.0, |p| p.triples as f64);
            let subjects_in = |cs: usize| sets[cs].subjects.max(1) as f64;
            let mut msg = vec![0.0; sets.len()];
            if l.from == node {
                // Node is the subject side: per subject set, the object-side rows each `p` triple reaches.
                let child = self.absorb(l.to, Some(li), groups, links, vectors)?;
                for &(cs, co, n) in &table.pairs {
                    let (cs, co) = (cs as usize, co as usize);
                    msg[cs] += n as f64 * child[co] / subjects_in(co);
                }
                if table.other > 0 {
                    let per = table.other as f64 * child.iter().sum::<f64>() / subject_count / total_triples;
                    for (cs, m) in msg.iter_mut().enumerate() {
                        *m += per * triples_in(cs);
                    }
                }
                for (cs, x) in w.iter_mut().enumerate() {
                    let t = triples_in(cs);
                    *x = if *x > 0.0 && t > 0.0 { cardinal(*x * msg[cs] / t) } else { 0.0 };
                }
            } else {
                // Node is the object side: per object set, the subject-side rows whose `p` triples reach it.
                let child = self.absorb(l.from, Some(li), groups, links, vectors)?;
                for &(cs, co, n) in &table.pairs {
                    let (cs, co) = (cs as usize, co as usize);
                    let t = triples_in(cs);
                    if t > 0.0 {
                        msg[co] += n as f64 * child[cs] / t;
                    }
                }
                if table.other > 0 {
                    let per = table.other as f64 * child.iter().sum::<f64>() / total_triples / subject_count;
                    for (co, m) in msg.iter_mut().enumerate() {
                        *m += per * sets[co].subjects as f64;
                    }
                }
                for (co, x) in w.iter_mut().enumerate() {
                    *x = if *x > 0.0 { cardinal(*x * msg[co] / subjects_in(co)) } else { 0.0 };
                }
            }
        }
        Some(w)
    }
    /// Compile star-subset estimates once for a bounded DP query. Each CS property
    /// is looked up once per pattern; subset products reuse the same ascending
    /// multiplication/division order as estimate_star, preserving its rounding.
    pub(crate) fn for_planning(&self, patterns: &[TriplePattern]) -> Self {
        let mut prepared = self.clone();
        prepared.star_estimates.clear();
        prepared.planning_domains.clear();
        for p in patterns {
            prepared
                .planning_domains
                .insert(*p, self.variable_domains(&[*p]));
        }
        let mut groups: BTreeMap<(u8, u64), Vec<TriplePattern>> = BTreeMap::new();
        for p in patterns {
            let key = match p.s {
                Slot::Var(v) => (0, u64::from(v)),
                Slot::Const(id) => (1, id.0),
            };
            let group = groups.entry(key).or_default();
            if !group.contains(p) {
                group.push(*p);
            }
        }
        for group in groups.into_values() {
            let n = group.len();
            if !(2..=10).contains(&n) || group.iter().any(|p| !matches!(p.p, Slot::Const(_))) {
                continue;
            }
            let mut objects = BTreeSet::new();
            if group
                .iter()
                .any(|p| matches!(p.o, Slot::Var(v) if p.s == p.o || !objects.insert(v)))
            {
                continue;
            }
            if matches!(group[0].s, Slot::Const(_))
                && group.iter().all(|p| self.exact_counts.contains_key(p))
            {
                continue;
            }
            let full = 1usize << n;
            let mut totals = vec![0.0; full];
            let mut products = vec![0.0; full];
            let mut matching = vec![0.0; n];
            for (cs_index, cs) in self.characteristic_sets.iter().enumerate() {
                let mut present = 0usize;
                for (i, p) in group.iter().enumerate() {
                    let Slot::Const(pred) = p.p else {
                        unreachable!()
                    };
                    matching[i] = cs.properties.get(&pred).map_or(0.0, |prop| match p.o {
                        Slot::Const(o) => self.exact_object_cs_counts.get(p).map_or_else(
                            || prop.frequencies.estimate(o),
                            |counts| counts[cs_index] as f64,
                        ),
                        Slot::Var(_) => prop.triples as f64,
                    });
                    if matching[i] > 0.0 {
                        present |= 1 << i;
                    }
                }
                if present.count_ones() < 2 {
                    continue;
                }
                products[0] = cs.subjects as f64;
                for mask in 1usize..full {
                    if mask & !present != 0 {
                        continue;
                    }
                    let i = (usize::BITS - 1 - mask.leading_zeros()) as usize;
                    let prior = mask ^ (1 << i);
                    products[mask] = cardinal(products[prior] * matching[i] / cs.subjects as f64);
                    if mask.count_ones() >= 2 {
                        totals[mask] = cardinal(totals[mask] + products[mask]);
                    }
                }
            }
            for (mask, total) in totals.into_iter().enumerate().skip(1) {
                if mask.count_ones() < 2 {
                    continue;
                }
                let key = group
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| mask & (1 << i) != 0)
                    .map(|(_, p)| *p)
                    .collect();
                let rows = if matches!(group[0].s, Slot::Const(_)) {
                    total / self.subject_count.max(1) as f64
                } else {
                    total
                };
                prepared.star_estimates.insert(key, cardinal(rows));
            }
        }
        prepared
    }
    pub(crate) fn variable_domains(&self, patterns: &[TriplePattern]) -> BTreeMap<u32, f64> {
        let mut result: BTreeMap<u32, f64> = BTreeMap::new();
        for p in patterns {
            if let Some(domains) = self.planning_domains.get(p) {
                for (&var, &domain) in domains {
                    result
                        .entry(var)
                        .and_modify(|old| *old = old.min(domain))
                        .or_insert(domain);
                }
                continue;
            }
            let property = if let Slot::Const(id) = p.p {
                self.predicates.get(&id)
            } else {
                None
            };
            for (i, slot) in slots(p).into_iter().enumerate() {
                if let Slot::Var(v) = slot {
                    let domain = match i {
                        0 => property.map_or(self.subject_count, |p| p.subjects) as f64,
                        1 => self.predicates.len() as f64,
                        _ => property.map_or_else(|| self.objects.count(), Property::ndv_objects),
                    }
                    .max(1.0);
                    result
                        .entry(v)
                        .and_modify(|x| *x = x.min(domain))
                        .or_insert(domain);
                }
            }
        }
        result
    }
}

/// A subject-to-object join between two stars of a BGP: pattern `pattern` of group `from` has as object the
/// subject of group `to`.
#[derive(Clone, Copy, Debug)]
struct Link {
    from: usize,
    pattern: usize,
    to: usize,
}

#[derive(Default)]
struct Builder {
    stats: Stats,
    last: Option<[TermId; 3]>,
    current: Vec<[TermId; 3]>,
    cs_index: BTreeMap<Vec<TermId>, usize>,
    /// Every subject in SPO order, and its characteristic set (for the characteristic-pair pass).
    subjects: Vec<TermId>,
    subject_sets: Vec<u32>,
}
impl Builder {
    fn push(&mut self, t: [TermId; 3]) -> Result<(), StatsError> {
        if let Some(last) = self.last {
            if t < last {
                return Err(StatsError::UnsortedSpo);
            }
            if t == last {
                self.stats.duplicate_triples_ignored += 1;
                return Ok(());
            }
            if t[0] != last[0] {
                self.flush_subject();
            }
        }
        let new_predicate_subject = self
            .last
            .is_none_or(|last| last[0] != t[0] || last[1] != t[1]);
        let prop = Arc::make_mut(&mut self.stats.predicates)
            .entry(t[1])
            .or_default();
        prop.add(t);
        if new_predicate_subject {
            prop.subjects += 1;
        }
        self.stats.triple_count += 1;
        self.stats.objects.add(t[2]);
        self.stats.equality[equality_mask(t)] += 1;
        self.current.push(t);
        self.last = Some(t);
        Ok(())
    }
    fn flush_subject(&mut self) {
        if self.current.is_empty() {
            return;
        }
        self.stats.subject_count += 1;
        let key: Vec<_> = self
            .current
            .iter()
            .map(|t| t[1])
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let index = *self.cs_index.entry(key.clone()).or_insert_with(|| {
            Arc::make_mut(&mut self.stats.characteristic_sets).push(CharacteristicSet {
                predicates: key,
                subjects: 0,
                properties: BTreeMap::new(),
            });
            self.stats.characteristic_sets.len() - 1
        });
        self.subjects.push(self.current[0][0]);
        self.subject_sets.push(index as u32);
        let cs = &mut Arc::make_mut(&mut self.stats.characteristic_sets)[index];
        cs.subjects += 1;
        for t in self.current.drain(..) {
            cs.properties.entry(t[1]).or_default().add(t);
        }
    }
    /// The statistics without pairs, and the pair pass seeded with every subject's set.
    fn finish(mut self) -> (Stats, PairBuilder) {
        self.flush_subject();
        self.stats.cs_lookup = Arc::new(
            self.stats
                .characteristic_sets
                .iter()
                .enumerate()
                .map(|(i, cs)| (cs.predicates.clone(), i))
                .collect(),
        );
        let pairs = PairBuilder {
            subjects: self.subjects,
            subject_sets: self.subject_sets,
            ..PairBuilder::default()
        };
        (self.stats, pairs)
    }
}

/// Second pass over the same sorted SPO input: counts characteristic pairs.
#[derive(Default)]
struct PairBuilder {
    subjects: Vec<TermId>,
    subject_sets: Vec<u32>,
    /// Position of the current subject in `subjects` (the pass visits subjects in the same order).
    at: usize,
    last: Option<[TermId; 3]>,
    counts: HashMap<(TermId, u32, u32), u64>,
    other: BTreeMap<TermId, u64>,
}
impl PairBuilder {
    fn push(&mut self, t: [TermId; 3]) {
        if self.last == Some(t) {
            return;
        }
        self.last = Some(t);
        while self.at < self.subjects.len() && self.subjects[self.at] < t[0] {
            self.at += 1;
        }
        // The first pass saw the same input, so the subject is always found; skip rather than panic if not.
        if self.subjects.get(self.at) != Some(&t[0]) {
            return;
        }
        let Ok(o) = self.subjects.binary_search(&t[2]) else {
            return;
        };
        let key = (t[1], self.subject_sets[self.at], self.subject_sets[o]);
        if let Some(n) = self.counts.get_mut(&key) {
            *n += 1;
        } else if self.counts.len() < MAX_PAIR_KEYS {
            self.counts.insert(key, 1);
        } else {
            *self.other.entry(t[1]).or_default() += 1;
        }
    }
    /// Keep the `PAIRS_PER_PREDICATE` largest pairs of each predicate (ties by set indices), the rest into `other`.
    fn finish(self) -> BTreeMap<TermId, PredicatePairs> {
        let mut by_predicate: BTreeMap<TermId, Vec<(u32, u32, u64)>> = BTreeMap::new();
        for ((p, cs, co), n) in self.counts {
            by_predicate.entry(p).or_default().push((cs, co, n));
        }
        let mut out: BTreeMap<TermId, PredicatePairs> = self
            .other
            .into_iter()
            .map(|(p, other)| (p, PredicatePairs { pairs: Vec::new(), other }))
            .collect();
        for (p, mut pairs) in by_predicate {
            pairs.sort_by(|a, b| b.2.cmp(&a.2).then((a.0, a.1).cmp(&(b.0, b.1))));
            let rest: u64 = pairs.iter().skip(PAIRS_PER_PREDICATE).map(|x| x.2).sum();
            pairs.truncate(PAIRS_PER_PREDICATE);
            pairs.sort_by_key(|x| (x.0, x.1));
            let entry = out.entry(p).or_default();
            entry.pairs = pairs;
            entry.other += rest;
        }
        out
    }
}

pub(crate) fn slots(p: &TriplePattern) -> [Slot; 3] {
    [p.s, p.p, p.o]
}
pub(crate) fn variables(p: &TriplePattern) -> BTreeSet<u32> {
    slots(p)
        .into_iter()
        .filter_map(|s| if let Slot::Var(v) = s { Some(v) } else { None })
        .collect()
}
pub(crate) fn cardinal(n: f64) -> f64 {
    if n.is_nan() || n < 0.0 {
        0.0
    } else {
        n.min(MAX_CARD)
    }
}
fn equality_mask(t: [TermId; 3]) -> usize {
    usize::from(t[0] == t[1]) | (usize::from(t[0] == t[2]) << 1) | (usize::from(t[1] == t[2]) << 2)
}
fn variable_equality_mask(p: &TriplePattern) -> usize {
    let s = slots(p);
    let eq = |a, b| matches!((s[a],s[b]),(Slot::Var(x),Slot::Var(y)) if x==y);
    usize::from(eq(0, 1)) | (usize::from(eq(0, 2)) << 1) | (usize::from(eq(1, 2)) << 2)
}
pub(crate) fn has_repeated_variable(p: &TriplePattern) -> bool {
    variable_equality_mask(p) != 0
}

/// Constants first, then preferred sort variable (for an index-ordered merge input).
pub fn scan_permutation(pattern: &TriplePattern, preferred: Option<u32>) -> Perm {
    Perm::ALL
        .into_iter()
        .max_by_key(|perm| {
            let s = slots(pattern);
            let order = perm.order();
            let constants = order
                .iter()
                .take_while(|i| matches!(s[**i], Slot::Const(_)))
                .count();
            let sorted = order.iter().skip(constants).find_map(|i| {
                if let Slot::Var(v) = s[*i] {
                    Some(v)
                } else {
                    None
                }
            });
            (
                constants,
                u8::from(preferred.is_some() && preferred == sorted),
            )
        })
        .unwrap()
}

/// Zero-aware q-error. A false zero (either direction) is infinite, never hidden by a +1 floor.
pub fn q_error(estimate: f64, actual: u64) -> f64 {
    if !estimate.is_finite() || estimate < 0.0 {
        return f64::INFINITY;
    }
    if estimate == 0.0 && actual == 0 {
        1.0
    } else if estimate == 0.0 || actual == 0 {
        f64::INFINITY
    } else {
        (estimate / actual as f64).max(actual as f64 / estimate)
    }
}

#[cfg(test)]
mod sketch_tests {
    use super::*;
    #[test]
    fn compiled_star_subsets_preserve_estimate_bits() {
        for seed in 0..12u64 {
            let mut triples = Vec::new();
            for subject in 1..=80u64 {
                for pred in 10..=15u64 {
                    if (subject * 3 + pred + seed) % 5 == 0 {
                        continue;
                    }
                    for degree in 0..1 + (subject + pred + seed) % 3 {
                        triples.push([
                            TermId(subject),
                            TermId(pred),
                            TermId(100 + (subject + degree + seed) % 7),
                        ]);
                    }
                }
            }
            triples.sort();
            triples.dedup();
            let stats = Stats::from_spo(triples).unwrap();
            let patterns: Vec<_> = (0..6u32)
                .map(|i| TriplePattern {
                    s: Slot::Var(0),
                    p: Slot::Const(TermId(10 + u64::from(i))),
                    o: if i == 3 {
                        Slot::Const(TermId(100 + seed % 7))
                    } else {
                        Slot::Var(i + 1)
                    },
                })
                .collect();
            let compiled = stats.for_planning(&patterns);
            for mask in 1usize..1 << patterns.len() {
                let subset: Vec<_> = patterns
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| mask & (1 << i) != 0)
                    .map(|(_, p)| *p)
                    .collect();
                assert_eq!(
                    stats.estimate_bgp(&subset).to_bits(),
                    compiled.estimate_bgp(&subset).to_bits(),
                    "seed={seed} mask={mask}"
                );
            }
        }
    }
    fn set_of(stats: &Stats, predicates: &[u64]) -> u32 {
        let key: Vec<_> = predicates.iter().map(|&p| TermId(p)).collect();
        stats.cs_lookup[&key] as u32
    }
    #[test]
    fn characteristic_pairs_count_linked_triples_by_set() {
        let mut t: Vec<[TermId; 3]> = Vec::new();
        let mut add = |s: u64, p: u64, o: u64| t.push([TermId(s), TermId(p), TermId(o)]);
        // Products 1..=4 {10 type, 11 producer}; producers 101, 102 {12 country}; reviews 201..=206
        // {13 reviewFor, 14 reviewer}; reviewers 301, 302 {12 country, 15 name}.
        for i in 1..=4 { add(i, 10, 900); add(i, 11, 100 + 1 + i % 2); }
        for i in 101..=102 { add(i, 12, 950); }
        for i in 201..=206 { add(i, 13, 1 + i % 4); add(i, 14, 301 + i % 2); }
        for i in 301..=302 { add(i, 12, 951); add(i, 15, 990 + i); }
        t.sort();
        let st = Stats::from_spo(t).unwrap();
        let (product, producer, review, reviewer) =
            (set_of(&st, &[10, 11]), set_of(&st, &[12]), set_of(&st, &[13, 14]), set_of(&st, &[12, 15]));
        let pairs = |p: u64| st.char_pairs(TermId(p)).cloned();
        assert_eq!(pairs(11), Some(PredicatePairs { pairs: vec![(product, producer, 4)], other: 0 }));
        assert_eq!(pairs(13), Some(PredicatePairs { pairs: vec![(review, product, 6)], other: 0 }));
        assert_eq!(pairs(14), Some(PredicatePairs { pairs: vec![(review, reviewer, 6)], other: 0 }));
        // Objects that are never subjects (classes, countries, names) have no set: no pairs.
        for p in [10, 12, 15] { assert_eq!(pairs(p), None, "{p}") }
        // The pairs survive persistence.
        let back = Stats::decode(&st.encode(), st.triple_count).unwrap();
        assert_eq!(back.char_pairs, st.char_pairs);
    }
    #[test]
    fn characteristic_pairs_keep_the_top_k_and_bucket_the_rest() {
        let n = PAIRS_PER_PREDICATE as u64 + 6;
        let mut t: Vec<[TermId; 3]> = Vec::new();
        let mut next = 10_000;
        for i in 0..n {
            // Object 1000 + i has its own set {2000 + i}; i + 1 linking subjects (all of set {5}) point at it.
            t.push([TermId(1000 + i), TermId(2000 + i), TermId(1)]);
            for _ in 0..=i {
                t.push([TermId(next), TermId(5), TermId(1000 + i)]);
                next += 1;
            }
        }
        t.sort();
        let st = Stats::from_spo(t).unwrap();
        let pp = st.char_pairs(TermId(5)).unwrap();
        assert_eq!(pp.pairs.len(), PAIRS_PER_PREDICATE);
        // The six smallest pairs (1 + 2 + ... + 6 triples) are bucketed.
        assert_eq!(pp.other, 21);
        let linking = set_of(&st, &[5]);
        let mut kept: Vec<u64> = pp.pairs.iter().map(|&(cs, co, c)| {
            assert_eq!(cs, linking);
            let i = st.characteristic_sets[co as usize].predicates[0].0 - 2000;
            assert_eq!(c, i + 1);
            c
        }).collect();
        kept.sort();
        assert_eq!(kept, (7..=n).collect::<Vec<_>>());
        assert!(pp.pairs.windows(2).all(|w| (w[0].0, w[0].1) < (w[1].0, w[1].1)));
        assert_eq!(pp.pairs.iter().map(|x| x.2).sum::<u64>() + pp.other, n * (n + 1) / 2);
    }
    #[test]
    fn hll_is_bounded_and_reasonable_for_tagged_sequential_terms() {
        let mut d = Distinct::default();
        assert_eq!(d.count(), 0.0);
        for i in 0..10_000 {
            d.add(TermId((3u64 << 60) | i));
        }
        assert!((7_000.0..13_000.0).contains(&d.count()), "{}", d.count());
    }
}

// ---------- persistence (task #34): the scan-derived core, without the lazily filled query caches ----------

/// Byte-format and statistics-semantics version. Bump when either the layout or the meaning of a field changes.
/// Version 2 is version 1's layout followed by the characteristic pairs:
/// `u64 n`, then per predicate (ascending) `u64 predicate, u64 other, u64 k`, then `k` times
/// `u64 subject set, u64 object set, u64 triples` (ascending by sets).
pub const STATS_FORMAT: &[u8; 8] = b"TLSTATS2";
/// The previous version, without characteristic pairs. It still decodes (with no pairs: the pre-pairs estimates).
pub const STATS_FORMAT_V1: &[u8; 8] = b"TLSTATS1";
/// Version of what the statistics scan computes (estimators, sketches, pairs), separate from the byte format.
/// Persisted statistics carry it and a file with another version is ignored, so a binary never plans with statistics
/// it would not have built itself. **Bump it in any change to what `from_scans` computes.**
pub const STATS_ALGO_VERSION: u32 = 1;

/// Why a persisted statistics payload was rejected (the caller then rescans; never a query error).
#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError { BadMagic, Truncated, Malformed(&'static str), TrailingBytes }

/// Upper bounds checked before allocating while decoding (a valid checksum does not make a payload safe).
const MAX_SETS: u64 = 1 << 32;
const MAX_PREDICATES: u64 = 1 << 32;

struct Writer(Vec<u8>);
impl Writer {
    fn u64(&mut self, v: u64) { self.0.extend_from_slice(&v.to_le_bytes()) }
    fn distinct(&mut self, d: &Distinct) { self.0.extend_from_slice(&d.registers) }
    fn property(&mut self, p: &Property) {
        self.u64(p.triples); self.u64(p.subjects); self.distinct(&p.objects);
        self.u64(p.frequencies.rounds); self.u64(p.frequencies.counts.len() as u64);
        for (id, c) in &p.frequencies.counts { self.u64(id.0); self.u64(*c) }
        for e in p.equality { self.u64(e) }
    }
    fn properties(&mut self, m: &BTreeMap<TermId, Property>) {
        self.u64(m.len() as u64);
        for (id, p) in m { self.u64(id.0); self.property(p) }
    }
    fn pairs(&mut self, m: &BTreeMap<TermId, PredicatePairs>) {
        self.u64(m.len() as u64);
        for (id, pp) in m {
            self.u64(id.0); self.u64(pp.other); self.u64(pp.pairs.len() as u64);
            for &(cs, co, n) in &pp.pairs { self.u64(u64::from(cs)); self.u64(u64::from(co)); self.u64(n) }
        }
    }
}

struct Reader<'a> { buf: &'a [u8], at: usize, max_entries: u64 }
impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Result<&[u8], DecodeError> {
        let end = self.at.checked_add(n).ok_or(DecodeError::Truncated)?;
        let s = self.buf.get(self.at..end).ok_or(DecodeError::Truncated)?;
        self.at = end;
        Ok(s)
    }
    fn u64(&mut self) -> Result<u64, DecodeError> { Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap())) }
    /// A length that must fit both `max` and the bytes left at `min_bytes` per element.
    fn len(&mut self, max: u64, min_bytes: usize) -> Result<usize, DecodeError> {
        let n = self.u64()?;
        let left = (self.buf.len() - self.at) as u64;
        if n > max || n > self.max_entries || n.saturating_mul(min_bytes as u64) > left { return Err(DecodeError::Malformed("length exceeds payload or base")) }
        Ok(n as usize)
    }
    fn distinct(&mut self) -> Result<Distinct, DecodeError> {
        let mut registers = [0u8; 128];
        registers.copy_from_slice(self.bytes(128)?);
        if registers.iter().any(|&r| r > 58) { return Err(DecodeError::Malformed("HLL register out of range")) }
        Ok(Distinct { registers })
    }
    fn property(&mut self) -> Result<Property, DecodeError> {
        let (triples, subjects, objects, rounds) = (self.u64()?, self.u64()?, self.distinct()?, self.u64()?);
        let n = self.len(HISTOGRAM_SIZE as u64, 16)?;
        let mut counts = BTreeMap::new();
        for _ in 0..n {
            let (id, c) = (TermId(self.u64()?), self.u64()?);
            if c == 0 || counts.insert(id, c).is_some() { return Err(DecodeError::Malformed("frequency entry")) }
        }
        let mut equality = [0u64; 8];
        for e in &mut equality { *e = self.u64()? }
        Ok(Property { triples, subjects, objects, frequencies: Frequencies { counts, rounds }, equality })
    }
    /// The pair section, checked against the already decoded sets and predicates.
    fn pairs(&mut self, sets: &[CharacteristicSet], predicates: &BTreeMap<TermId, Property>) -> Result<BTreeMap<TermId, PredicatePairs>, DecodeError> {
        let bad = |what| DecodeError::Malformed(what);
        let n = self.len(MAX_PREDICATES, 24)?;
        let mut m = BTreeMap::new();
        for _ in 0..n {
            let id = TermId(self.u64()?);
            let other = self.u64()?;
            let k = self.len(PAIRS_PER_PREDICATE as u64, 24)?;
            let prop = predicates.get(&id).ok_or(bad("pairs of an unknown predicate"))?;
            let mut pairs = Vec::with_capacity(k);
            let mut by_subject_set: BTreeMap<u32, u64> = BTreeMap::new();
            let mut total = other;
            for _ in 0..k {
                let (cs, co, c) = (self.u64()?, self.u64()?, self.u64()?);
                let (Ok(cs), Ok(co)) = (u32::try_from(cs), u32::try_from(co)) else { return Err(bad("pair set index")) };
                if cs as usize >= sets.len() || co as usize >= sets.len() || c == 0 { return Err(bad("pair entry")) }
                if pairs.last().is_some_and(|&(a, b, _)| (a, b) >= (cs, co)) { return Err(bad("pairs not sorted")) }
                let in_set = sets[cs as usize].properties.get(&id).ok_or(bad("pair subject set lacks the predicate"))?.triples;
                let s = by_subject_set.entry(cs).or_default();
                *s = s.checked_add(c).ok_or(bad("overflow"))?;
                if *s > in_set { return Err(bad("pairs exceed the set's triples")) }
                total = total.checked_add(c).ok_or(bad("overflow"))?;
                pairs.push((cs, co, c));
            }
            if total == 0 || total > prop.triples { return Err(bad("pair totals")) }
            if m.insert(id, PredicatePairs { pairs, other }).is_some() { return Err(bad("duplicate pair predicate")) }
        }
        Ok(m)
    }
    fn properties(&mut self) -> Result<BTreeMap<TermId, Property>, DecodeError> {
        let n = self.len(MAX_PREDICATES, 8 + 8 * 4 + 128 + 64)?;
        let mut m = BTreeMap::new();
        for _ in 0..n {
            let id = TermId(self.u64()?);
            let p = self.property()?;
            if m.insert(id, p).is_some() { return Err(DecodeError::Malformed("duplicate predicate")) }
        }
        Ok(m)
    }
}

impl Stats {
    /// The scan-derived core in a self-describing little-endian encoding (magic/version first). The query-time
    /// caches (exact counts, star estimates, planning domains) are not included; they refill on demand.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = self.encode_core(STATS_FORMAT);
        w.pairs(&self.char_pairs);
        w.0
    }
    /// A version-1 payload (no pairs), as statistics files written before version 2 hold.
    #[cfg(test)]
    pub(crate) fn encode_v1(&self) -> Vec<u8> {
        self.encode_core(STATS_FORMAT_V1).0
    }
    fn encode_core(&self, magic: &[u8; 8]) -> Writer {
        let mut w = Writer(magic.to_vec());
        w.u64(self.triple_count); w.u64(self.subject_count); w.u64(self.duplicate_triples_ignored);
        w.distinct(&self.objects);
        for e in self.equality { w.u64(e) }
        w.properties(&self.predicates);
        w.u64(self.characteristic_sets.len() as u64);
        for cs in self.characteristic_sets.iter() {
            w.u64(cs.subjects);
            w.u64(cs.predicates.len() as u64);
            for p in &cs.predicates { w.u64(p.0) }
            w.properties(&cs.properties);
        }
        w
    }

    /// Inverse of `encode` for a base holding `base_triples` triples (from its own metadata, not the payload).
    /// Every length is bounded by that count before allocation, and the decoded summary must satisfy the scan's
    /// invariants (see `check_invariants`); malformed input is an error, never a panic. A version-1 payload decodes
    /// with no characteristic pairs.
    pub fn decode(buf: &[u8], base_triples: u64) -> Result<Self, DecodeError> {
        if buf.len() < 8 { return Err(DecodeError::BadMagic) }
        let with_pairs = if &buf[..8] == STATS_FORMAT { true } else if &buf[..8] == STATS_FORMAT_V1 { false } else { return Err(DecodeError::BadMagic) };
        let mut r = Reader { buf, at: 8, max_entries: base_triples };
        let (triple_count, subject_count, duplicate_triples_ignored) = (r.u64()?, r.u64()?, r.u64()?);
        let objects = r.distinct()?;
        let mut equality = [0u64; 8];
        for e in &mut equality { *e = r.u64()? }
        let predicates = r.properties()?;
        let n = r.len(MAX_SETS, 16)?;
        let mut sets = Vec::with_capacity(n);
        for _ in 0..n {
            let subjects = r.u64()?;
            let k = r.len(MAX_PREDICATES, 8)?;
            let mut preds = Vec::with_capacity(k);
            for _ in 0..k { preds.push(TermId(r.u64()?)) }
            if preds.windows(2).any(|w| w[0] >= w[1]) { return Err(DecodeError::Malformed("characteristic set not sorted")) }
            let properties = r.properties()?;
            sets.push(CharacteristicSet { predicates: preds, subjects, properties });
        }
        let char_pairs = if with_pairs { r.pairs(&sets, &predicates)? } else { BTreeMap::new() };
        if r.at != buf.len() { return Err(DecodeError::TrailingBytes) }
        let cs_lookup: HashMap<Vec<TermId>, usize> = sets.iter().enumerate().map(|(i, cs)| (cs.predicates.clone(), i)).collect();
        if cs_lookup.len() != sets.len() { return Err(DecodeError::Malformed("duplicate characteristic set")) }
        let st = Stats {
            triple_count, subject_count, duplicate_triples_ignored,
            characteristic_sets: Arc::new(sets), cs_lookup: Arc::new(cs_lookup), predicates: Arc::new(predicates),
            objects, equality, char_pairs: Arc::new(char_pairs), ..Default::default()
        };
        st.check_invariants(base_triples)?;
        Ok(st)
    }

    /// Relations every scan-built summary satisfies; a payload that violates one is rejected as malformed.
    fn check_invariants(&self, base_triples: u64) -> Result<(), DecodeError> {
        let bad = |what| Err(DecodeError::Malformed(what));
        // Misra-Gries bookkeeping: every add either raises one counter by 1 or (on overflow) lowers all
        // HISTOGRAM_SIZE counters by 1 and drops the value, so triples = sum(counts) + (HISTOGRAM_SIZE + 1) * rounds.
        let props_ok = |m: &BTreeMap<TermId, Property>| m.iter().all(|(id, p)| {
            id.0 != 0 && p.triples > 0 && p.subjects <= p.triples && p.equality.iter().try_fold(0u64, |a, &e| a.checked_add(e)) == Some(p.triples)
                && p.frequencies.counts.keys().all(|k| k.0 != 0)
                && p.frequencies.counts.values().try_fold(0u64, |a, &c| a.checked_add(c))
                    .and_then(|f| p.frequencies.rounds.checked_mul(HISTOGRAM_SIZE as u64 + 1).and_then(|r| f.checked_add(r))) == Some(p.triples)
        });
        if self.triple_count != base_triples { return bad("triple count differs from the base") }
        if self.subject_count > self.triple_count || self.duplicate_triples_ignored != 0 { return bad("subject or duplicate count") }
        if self.equality.iter().try_fold(0u64, |a, &e| a.checked_add(e)) != Some(self.triple_count) { return bad("equality buckets") }
        if !props_ok(&self.predicates) { return bad("predicate summary") }
        if self.predicates.values().try_fold(0u64, |a, p| a.checked_add(p.triples)) != Some(self.triple_count) { return bad("predicate totals") }
        if self.characteristic_sets.iter().try_fold(0u64, |a, cs| a.checked_add(cs.subjects)) != Some(self.subject_count) { return bad("characteristic-set subjects") }
        // Per predicate: its triples split exactly across the characteristic sets containing it, and its subjects
        // are exactly those sets' subjects. Each set's property map has exactly the set's predicates.
        let mut triples_by_p: BTreeMap<TermId, u64> = BTreeMap::new();
        let mut subjects_by_p: BTreeMap<TermId, u64> = BTreeMap::new();
        for cs in self.characteristic_sets.iter() {
            if cs.subjects == 0 || cs.predicates.is_empty() || cs.predicates.iter().any(|p| !self.predicates.contains_key(p)) { return bad("characteristic set") }
            if !props_ok(&cs.properties) || !cs.properties.keys().eq(cs.predicates.iter()) { return bad("characteristic-set properties") }
            for (p, prop) in &cs.properties {
                if prop.triples < cs.subjects || prop.subjects != 0 { return bad("characteristic-set property counts") }
                let t = triples_by_p.entry(*p).or_default();
                *t = t.checked_add(prop.triples).ok_or(DecodeError::Malformed("overflow"))?;
                let n = subjects_by_p.entry(*p).or_default();
                *n = n.checked_add(cs.subjects).ok_or(DecodeError::Malformed("overflow"))?;
            }
        }
        for (p, prop) in self.predicates.iter() {
            if triples_by_p.get(p) != Some(&prop.triples) || subjects_by_p.get(p) != Some(&prop.subjects) { return bad("characteristic sets disagree with predicate summary") }
        }
        // Finite, positive estimates follow from the above; check the derived NDVs explicitly anyway.
        if !self.objects.count().is_finite() || self.predicates.values().any(|p| !p.ndv_objects().is_finite()) { return bad("non-finite estimate") }
        Ok(())
    }
}

#[cfg(test)]
mod persist_tests {
    use super::*;
    fn sample() -> Stats {
        let mut t: Vec<[TermId; 3]> = Vec::new();
        for s in 1..200u64 {
            t.push([TermId(s), TermId(1000), TermId(5000 + s % 7)]);
            if s % 3 == 0 { t.push([TermId(s), TermId(1001), TermId(s)]) }
            if s % 5 == 0 { t.push([TermId(s), TermId(1002), TermId(9000 + s)]) }
        }
        t.sort();
        Stats::from_spo(t).unwrap()
    }
    #[test]
    fn roundtrip_preserves_core_and_estimates() {
        let st = sample();
        let bytes = st.encode();
        let back = Stats::decode(&bytes, st.triple_count).unwrap();
        assert_eq!(back.encode(), bytes);
        assert_eq!((back.triple_count, back.subject_count), (st.triple_count, st.subject_count));
        let v = |i| Slot::Var(i);
        let c = |x| Slot::Const(TermId(x));
        for p in [TriplePattern { s: v(0), p: c(1000), o: v(1) }, TriplePattern { s: v(0), p: c(1001), o: v(0) },
                  TriplePattern { s: v(0), p: c(1000), o: c(5003) }, TriplePattern { s: v(0), p: v(1), o: v(2) }] {
            assert_eq!(st.estimate_pattern(&p).rows.to_bits(), back.estimate_pattern(&p).rows.to_bits(), "{p:?}");
        }
        let bgp = [TriplePattern { s: v(0), p: c(1000), o: v(1) }, TriplePattern { s: v(0), p: c(1002), o: v(2) }];
        assert_eq!(st.estimate_bgp(&bgp).to_bits(), back.estimate_bgp(&bgp).to_bits());
    }
    /// A per-query clone shares the immutable synopsis instead of copying it (`cache_pattern_counts` then fills only
    /// the clone's own caches).
    #[test]
    fn clone_shares_the_synopsis() {
        let st = sample();
        assert!(Arc::ptr_eq(&st.characteristic_sets, &st.clone().characteristic_sets));
    }
    #[test]
    fn version_1_payload_decodes_without_pairs() {
        let st = sample();
        assert!(st.uses_char_pairs(), "sample has linked triples (1001 points at its own subject)");
        let v1 = st.encode_v1();
        let back = Stats::decode(&v1, st.triple_count).unwrap();
        assert!(back.char_pairs.is_empty() && !back.uses_char_pairs());
        // Re-encoding writes version 2 with an empty pair section.
        assert_eq!(&back.encode()[..8], STATS_FORMAT);
        // Its estimates are the pre-pairs ones: those of the original with pairs switched off.
        let mut off = st.clone();
        off.set_char_pairs(false);
        let v = |i| Slot::Var(i);
        let c = |x| Slot::Const(TermId(x));
        for bgp in [
            vec![TriplePattern { s: v(0), p: c(1000), o: v(1) }, TriplePattern { s: v(0), p: c(1002), o: v(2) }],
            vec![TriplePattern { s: v(0), p: c(1001), o: v(1) }, TriplePattern { s: v(1), p: c(1000), o: c(5003) }],
        ] {
            assert_eq!(off.estimate_bgp(&bgp).to_bits(), back.estimate_bgp(&bgp).to_bits(), "{bgp:?}");
        }
        // A version-1 payload with bytes after it is still rejected.
        let mut extra = v1.clone(); extra.push(0);
        assert_eq!(Stats::decode(&extra, st.triple_count).unwrap_err(), DecodeError::TrailingBytes);
    }
    #[test]
    fn malformed_pairs_are_rejected() {
        let st = sample();
        let n_t = st.triple_count;
        let (&p, pp) = st.char_pairs.iter().next().unwrap();
        let mut too_many = st.clone();
        Arc::make_mut(&mut too_many.char_pairs).get_mut(&p).unwrap().other = st.predicates[&p].triples + 1;
        assert!(matches!(Stats::decode(&too_many.encode(), n_t), Err(DecodeError::Malformed(_))));
        let mut bad_set = st.clone();
        Arc::make_mut(&mut bad_set.char_pairs).get_mut(&p).unwrap().pairs[0].1 = st.characteristic_sets.len() as u32;
        assert!(matches!(Stats::decode(&bad_set.encode(), n_t), Err(DecodeError::Malformed(_))));
        let mut unknown = st.clone();
        Arc::make_mut(&mut unknown.char_pairs).insert(TermId(424242), pp.clone());
        assert!(matches!(Stats::decode(&unknown.encode(), n_t), Err(DecodeError::Malformed(_))));
    }
    #[test]
    fn malformed_payloads_are_errors_not_panics() {
        let st = sample();
        let n_t = st.triple_count;
        let bytes = st.encode();
        for n in 0..bytes.len() { assert!(Stats::decode(&bytes[..n], n_t).is_err(), "prefix {n}") }
        let mut extra = bytes.clone(); extra.push(0);
        assert_eq!(Stats::decode(&extra, n_t).unwrap_err(), DecodeError::TrailingBytes);
        let mut bad = bytes.clone(); bad[0] ^= 1;
        assert_eq!(Stats::decode(&bad, n_t).unwrap_err(), DecodeError::BadMagic);
        // A huge declared length must be refused before allocating.
        let mut huge = bytes[..8 + 24 + 128 + 64].to_vec(); huge.extend_from_slice(&u64::MAX.to_le_bytes());
        assert!(Stats::decode(&huge, n_t).is_err());
        // A well-formed payload for a different base (wrong triple count) violates the invariants.
        assert!(matches!(Stats::decode(&bytes, n_t + 1), Err(DecodeError::Malformed(_))));
        // Well-formed bytes with an inconsistent total (as a checksum-valid but wrong file would carry).
        let mut skew = st.clone(); skew.subject_count += 1;
        assert!(matches!(Stats::decode(&skew.encode(), n_t), Err(DecodeError::Malformed(_))));
        let mut skew = st.clone(); skew.equality[0] += 1;
        assert!(matches!(Stats::decode(&skew.encode(), n_t), Err(DecodeError::Malformed(_))));
        // Jianlin's control: a predicate's frequency table cleared without rounds would report an exact zero for an
        // object that exists; the Misra-Gries total must reject it.
        let mut cleared = st.clone();
        Arc::make_mut(&mut cleared.predicates).get_mut(&TermId(1000)).unwrap().frequencies.counts.clear();
        assert!(matches!(Stats::decode(&cleared.encode(), n_t), Err(DecodeError::Malformed(_))));
        // An empty characteristic-set property map, and a set property whose totals disagree with the predicate.
        let mut empty_cs = st.clone();
        Arc::make_mut(&mut empty_cs.characteristic_sets)[0].properties.clear();
        assert!(matches!(Stats::decode(&empty_cs.encode(), n_t), Err(DecodeError::Malformed(_))));
        let mut moved = st.clone();
        let p = *moved.characteristic_sets[0].predicates.first().unwrap();
        { let prop = Arc::make_mut(&mut moved.characteristic_sets)[0].properties.get_mut(&p).unwrap();
          prop.triples += 1; prop.equality[0] += 1; prop.frequencies.rounds = 0;
          prop.frequencies.counts = BTreeMap::from([(TermId(1), prop.triples)]); }
        assert!(matches!(Stats::decode(&moved.encode(), n_t), Err(DecodeError::Malformed(_))));
        // Single-byte corruption anywhere: never a panic (it may still decode if it hits a plain counter).
        for i in 8..bytes.len() { let mut b = bytes.clone(); b[i] ^= 0xff; let _ = Stats::decode(&b, n_t); }
    }
}
