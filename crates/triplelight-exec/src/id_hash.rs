//! Hash maps and sets for the executor's per-row paths: filter memos, group keys, DISTINCT, join tables. They are
//! probed once per row (millions of rows on a large scan), and the standard SipHash costs more than the work a probe
//! saves. foldhash is a few cycles per key and, like SipHash, is seeded at random per map: some IDs carry client bytes
//! (the in-memory store inlines short strings into the ID), and a per-map seed stops collisions computed in advance
//! against a fixed function. foldhash is not a cryptographic hash, so it promises nothing more than that.
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use foldhash::fast::{FoldHasher, SeedableRandomState};
use foldhash::SharedSeed;

use crate::model::TermId;

/// A `HashMap` hashed with seeded foldhash.
pub(crate) type FastMap<K, V> = HashMap<K, V, IdState>;
/// A `HashSet` hashed with seeded foldhash.
pub(crate) type FastSet<K> = HashSet<K, IdState>;
/// A map keyed by term ID, hashed with seeded foldhash.
pub(crate) type IdMap<V> = FastMap<TermId, V>;

/// An empty [`FastSet`] with room for `n` entries.
pub(crate) fn fast_set<K>(n: usize) -> FastSet<K> { FastSet::with_capacity_and_hasher(n, Default::default()) }

/// foldhash's fast variant, with a process-wide shared seed that has passed [`spreads`]. foldhash hashes one integer
/// with a single folded multiply whose multiplier comes from the shared seed, and for about 1% of random shared seeds
/// the top bits of neighbouring integers collapse: hashbrown's 7-bit control byte then takes a few dozen values
/// instead of 128, and probes compare far more keys (#105). Drawing that seed ourselves and redrawing a bad one costs
/// a few microseconds once per process, and the hash itself is unchanged. The shared seed and every per-map seed stay
/// random per process.
#[derive(Clone, Debug)]
pub(crate) struct IdState(SeedableRandomState);

impl Default for IdState {
    #[inline]
    fn default() -> Self {
        let (base, shared) = shared();
        let n = NEXT_MAP.fetch_add(1, Ordering::Relaxed);
        IdState(SeedableRandomState::with_seed(base ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15), shared))
    }
}

impl BuildHasher for IdState {
    type Hasher = FoldHasher<'static>;
    #[inline(always)]
    fn build_hasher(&self) -> FoldHasher<'static> { self.0.build_hasher() }
}

static NEXT_MAP: AtomicU64 = AtomicU64::new(0);

/// The process's random per-map seed base and its checked shared seed, drawn on first use.
fn shared() -> (u64, &'static SharedSeed) {
    static SEED: OnceLock<(u64, SharedSeed)> = OnceLock::new();
    let (base, seed) = SEED.get_or_init(|| {
        let mut seed = SharedSeed::from_u64(random_u64());
        for _ in 0..MAX_DRAWS {
            if spreads(&seed) { break; }
            seed = SharedSeed::from_u64(random_u64());
        }
        (random_u64(), seed)
    });
    (*base, seed)
}

/// Redraws allowed for a bad shared seed. At a 1% failure rate the chance that all of them fail is 1e-32.
const MAX_DRAWS: usize = 16;

/// A random `u64` from the standard library's randomly keyed SipHash.
fn random_u64() -> u64 { std::hash::RandomState::new().build_hasher().finish() }

/// Whether 4,096 neighbouring IDs reach at least [`MIN_CONTROL`] of hashbrown's 128 control-byte values under `seed`.
fn spreads(seed: &SharedSeed) -> bool { control_values(&FoldHasher::with_seed(0, seed)) >= MIN_CONTROL }

/// Distinct top-7-bit values over 4,096 neighbouring IDs of one kind, hashed by clones of `proto`.
fn control_values(proto: &FoldHasher<'_>) -> usize {
    let mut top = [false; 128];
    for i in 0..4096u64 {
        let mut h = proto.clone();
        h.write_u64((3 << 60) | i);
        top[(h.finish() >> 57) as usize] = true;
    }
    top.iter().filter(|b| **b).count()
}

/// The control-byte coverage a shared seed must reach: the fixed seeds in the tests below all reach 128.
const MIN_CONTROL: usize = 120;

#[cfg(test)]
mod tests {
    use super::*;

    /// Distinct values of hashbrown's 7-bit control byte (the hash's top bits) and of its bucket index (the low
    /// bits, for a table of 4,096 buckets) over 4,096 neighbouring IDs, under one fixed per-hasher seed.
    fn spread(seed: u64) -> (usize, usize) {
        let build = foldhash::fast::FixedState::with_seed(seed);
        let (mut top, mut low) = ([false; 128], vec![false; 4096]);
        for i in 0..4096u64 {
            let h = build.hash_one(TermId((3 << 60) | i));
            top[(h >> 57) as usize] = true;
            low[(h & 4095) as usize] = true;
        }
        (top.iter().filter(|b| **b).count(), low.iter().filter(|b| **b).count())
    }

    /// foldhash's fast variant hashes one integer with a single folded multiply whose multiplier comes from the
    /// process-wide shared seed, and for about 1% of shared seeds the top bits of neighbouring integers collapse (#105:
    /// CI runs reached 54 and 97 of 128 control-byte values). A test on the process's random seed therefore flakes.
    /// This one is deterministic: the fixed shared seed and 256 fixed per-hasher seeds, against bounds measured on
    /// them (worst case 2,669 of 4,096 buckets, about what random placement gives, and all 128 control values).
    /// The maps themselves avoid the bad class by checking their shared seed ([`IdState`]).
    #[test]
    fn neighbouring_ids_spread_under_fixed_seeds() {
        let (mut worst_top, mut worst_low) = (128, 4096);
        for seed in 0..256 {
            let (top, low) = spread(seed);
            worst_top = worst_top.min(top);
            worst_low = worst_low.min(low);
        }
        assert!(worst_low >= MIN_BUCKETS, "bucket index: {worst_low} of 4096 buckets used, worst of 256 seeds");
        assert!(worst_top >= MIN_CONTROL, "control byte: {worst_top} of 128 values, worst of 256 seeds");
    }
    /// Measured worst case over these 256 fixed seeds, less a margin.
    const MIN_BUCKETS: usize = 2_400;

    #[test]
    fn the_process_shared_seed_spreads_neighbouring_ids() {
        assert!(control_values(&IdState::default().build_hasher()) >= MIN_CONTROL);
    }

    #[test]
    fn two_maps_hash_differently() {
        let (a, b) = (IdState::default(), IdState::default());
        let id = TermId((0xf << 60) | 0x0300_0000_0061_6263);
        assert!((0..8u64).any(|i| a.hash_one(TermId(id.0 + i)) != b.hash_one(TermId(id.0 + i))), "the seed is not random per map");
    }

    #[test]
    fn a_map_finds_what_it_holds() {
        let mut m: IdMap<u32> = IdMap::default();
        for i in 0..10_000u64 { m.insert(TermId(i * 7919), i as u32); }
        assert!((0..10_000u64).all(|i| m[&TermId(i * 7919)] == i as u32));
    }
}
