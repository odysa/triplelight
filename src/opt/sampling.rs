//! Fixed-size, inverse-probability random walks over a snapshot's prefix indexes.
//! Research API: does not alter the default executor or turn approximate zeros into empty results.
//! Based on Li et al., Wander Join, SIGMOD 2016, doi:10.1145/2882903.2915235.
use super::plan::{Slot, TriplePattern};
use crate::{
    model::{Perm, TermId},
    storage::{Snapshot, StorageError},
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub struct WalkOptions {
    pub walks: usize,
    pub seed: u64,
    pub max_probes: usize,
    pub max_time: Duration,
}
impl Default for WalkOptions {
    fn default() -> Self {
        Self {
            walks: 256,
            seed: 20261115,
            max_probes: 4096,
            max_time: Duration::from_micros(500),
        }
    }
}
#[derive(Clone, Debug)]
pub struct WalkEstimate {
    pub rows: f64,
    /// Descriptive standard error; a zero-success sample is NOT evidence of an empty join.
    pub standard_error: f64,
    pub successful_walks: usize,
    pub walks: usize,
    pub probes: usize,
    pub elapsed: Duration,
    /// Prefix means for the supplied fixed pattern order, useful for cost calibration.
    pub prefix_rows: Vec<f64>,
}
#[derive(Clone, Debug)]
pub enum WalkOutcome {
    Complete(WalkEstimate),
    /// All partial samples are discarded: keeping only fast completed walks biases estimates.
    BudgetExceeded {
        completed: usize,
        probes: usize,
        elapsed: Duration,
    },
    UnsupportedRank,
}
struct Random(u64);
impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        let threshold = n.wrapping_neg() % n;
        loop {
            let x = self.next();
            if x >= threshold {
                return x % n;
            }
        }
    }
}
fn invalid(message: &str) -> StorageError {
    StorageError::new(std::io::ErrorKind::InvalidInput, message)
}

/// Every successful path contributes the product of exact conditional degrees.
/// Failed paths contribute zero. A fixed order and sample size make this an unbiased
/// bag-cardinality estimator. Deadlines are cooperative between index operations;
/// one storage read can overrun them. Storage errors propagate unchanged.
pub fn estimate_walks(
    snapshot: &dyn Snapshot,
    patterns: &[TriplePattern],
    order: &[usize],
    options: WalkOptions,
) -> Result<WalkOutcome, StorageError> {
    if options.walks == 0 {
        return Err(invalid("walk count must be positive"));
    }
    let mut sorted = order.to_vec();
    sorted.sort_unstable();
    if sorted != (0..patterns.len()).collect::<Vec<_>>() {
        return Err(invalid("walk order must be a permutation"));
    }
    if !snapshot.has_fast_rank() {
        return Ok(WalkOutcome::UnsupportedRank);
    }
    let start = Instant::now();
    let mut probes = 0;
    let mut rng = Random(options.seed);
    let mut mean = 0.;
    let mut m2 = 0.;
    let mut success = 0;
    let mut prefix_sums = vec![0.; order.len()];
    for walk in 0..options.walks {
        let mut bindings = BTreeMap::new();
        let mut weight = 1.;
        let mut alive = true;
        for (step, &index) in order.iter().enumerate() {
            if !alive {
                break;
            }
            if probes + 2 > options.max_probes || start.elapsed() >= options.max_time {
                return Ok(WalkOutcome::BudgetExceeded {
                    completed: walk,
                    probes,
                    elapsed: start.elapsed(),
                });
            }
            let p = patterns[index];
            let slots = [p.s, p.p, p.o];
            let bound = slots.map(|s| match s {
                Slot::Const(id) => Some(id),
                Slot::Var(v) => bindings.get(&v).copied(),
            });
            let perm = Perm::ALL
                .into_iter()
                .max_by_key(|p| {
                    p.order()
                        .iter()
                        .take_while(|&&i| bound[i].is_some())
                        .count()
                })
                .unwrap();
            let prefix: Vec<TermId> = perm.order().iter().map_while(|&i| bound[i]).collect();
            probes += 1;
            let degree = snapshot.count(perm, &prefix)?;
            if degree == 0 {
                alive = false;
                weight = 0.;
                break;
            }
            probes += 1;
            let row = snapshot
                .row_at(perm, &prefix, rng.below(degree))?
                .ok_or_else(|| {
                    StorageError::new(
                        std::io::ErrorKind::InvalidData,
                        "rank/count disagree within snapshot",
                    )
                })?;
            let mut triple = [TermId(0); 3];
            for (key, position) in perm.order().into_iter().enumerate() {
                triple[position] = row[key];
            }
            weight *= degree as f64;
            if !weight.is_finite() {
                return Err(invalid("walk weight overflow"));
            }
            for (slot, value) in slots.into_iter().zip(triple) {
                match slot {
                    Slot::Const(c) if c != value => alive = false,
                    Slot::Var(v) => {
                        if let Some(old) = bindings.insert(v, value) {
                            if old != value {
                                alive = false;
                            }
                        }
                    }
                    _ => {}
                }
            }
            if !alive {
                weight = 0.;
                break;
            }
            prefix_sums[step] += weight;
        }
        if alive {
            success += 1;
        }
        let delta = weight - mean;
        mean += delta / (walk + 1) as f64;
        m2 += delta * (weight - mean);
        if start.elapsed() >= options.max_time {
            return Ok(WalkOutcome::BudgetExceeded {
                completed: walk + 1,
                probes,
                elapsed: start.elapsed(),
            });
        }
    }
    Ok(WalkOutcome::Complete(WalkEstimate {
        rows: mean,
        standard_error: if options.walks > 1 {
            (m2 / ((options.walks - 1) * options.walks) as f64)
                .max(0.)
                .sqrt()
        } else {
            f64::NAN
        },
        successful_walks: success,
        walks: options.walks,
        probes,
        elapsed: start.elapsed(),
        prefix_rows: prefix_sums
            .into_iter()
            .map(|s| s / options.walks as f64)
            .collect(),
    }))
}
