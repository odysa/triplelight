//! Hash maps and sets for the executor's per-row paths: filter memos, group keys, DISTINCT, join tables. They are
//! probed once per row (millions of rows on a large scan), and the standard SipHash costs more than the work a probe
//! saves. foldhash is a few cycles per key and, like SipHash, is seeded at random per map: some IDs carry client bytes
//! (the in-memory store inlines short strings into the ID), and a per-map seed stops collisions computed in advance
//! against a fixed function. foldhash is not a cryptographic hash, so it promises nothing more than that.
use std::collections::{HashMap, HashSet};

use crate::model::TermId;

/// A `HashMap` hashed with seeded foldhash.
pub(crate) type FastMap<K, V> = HashMap<K, V, foldhash::fast::RandomState>;
/// A `HashSet` hashed with seeded foldhash.
pub(crate) type FastSet<K> = HashSet<K, foldhash::fast::RandomState>;
/// A map keyed by term ID, hashed with seeded foldhash.
pub(crate) type IdMap<V> = FastMap<TermId, V>;

/// An empty [`FastSet`] with room for `n` entries.
pub(crate) fn fast_set<K>(n: usize) -> FastSet<K> { FastSet::with_capacity_and_hasher(n, Default::default()) }

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::BuildHasher;

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
    /// Whether the maps should switch to a variant without the bad seed class is measured separately.
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
    /// Measured worst cases over these 256 fixed seeds, less a margin.
    const MIN_BUCKETS: usize = 2_400;
    const MIN_CONTROL: usize = 120;

    #[test]
    fn two_maps_hash_differently() {
        let (a, b) = (foldhash::fast::RandomState::default(), foldhash::fast::RandomState::default());
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
