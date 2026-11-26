//! A hasher for maps keyed by one `TermId`. Filter memos look a term up once per row (millions of rows on a large
//! scan), and the standard SipHash costs more than the work the memo saves. foldhash is a few cycles per key and, like
//! SipHash, is seeded at random per map: some IDs carry client bytes (the in-memory store inlines short strings into
//! the ID), so a fixed function would let a client choose colliding keys.
use std::collections::HashMap;

use crate::model::TermId;

/// A map keyed by term ID, hashed with seeded foldhash.
pub(crate) type IdMap<V> = HashMap<TermId, V, foldhash::fast::RandomState>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::BuildHasher;

    #[test]
    fn neighbouring_ids_spread_over_the_top_bits() {
        let build = foldhash::fast::RandomState::default();
        let tops: std::collections::HashSet<u64> = (0..4096u64).map(|i| build.hash_one(TermId((3 << 60) | i)) >> 57).collect();
        assert_eq!(tops.len(), 128, "hashbrown's 7 control bits take every value");
    }

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
