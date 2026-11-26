//! A hasher for maps keyed by one `TermId`. Filter memos look a term up once per row (millions of rows on a large
//! scan), and the standard SipHash costs more than the work the memo saves. A `TermId` is a single `u64` the engine
//! assigned, not text a client controls, so a fixed mixing function is enough: the murmur3 64-bit finalizer spreads
//! every input bit over the output, which hashbrown needs because it takes its control bits from the top of the hash.
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use crate::model::TermId;

#[derive(Default, Clone, Copy)]
pub(crate) struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        let mut x = self.0;
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
        x ^= x >> 33;
        x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        x ^ (x >> 33)
    }
    fn write_u64(&mut self, n: u64) { self.0 = self.0.rotate_left(5) ^ n }
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.write_u64(u64::from_le_bytes(word));
        }
    }
}

/// A map keyed by term ID, hashed with [`IdHasher`].
pub(crate) type IdMap<V> = HashMap<TermId, V, BuildHasherDefault<IdHasher>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighbouring_ids_spread_over_the_top_bits() {
        use std::hash::BuildHasher;
        let build = BuildHasherDefault::<IdHasher>::default();
        let tops: std::collections::HashSet<u64> = (0..4096u64).map(|i| build.hash_one(TermId((3 << 60) | i)) >> 57).collect();
        assert_eq!(tops.len(), 128, "the 7 control bits take every value");
    }

    #[test]
    fn a_map_finds_what_it_holds() {
        let mut m: IdMap<u32> = IdMap::default();
        for i in 0..10_000u64 { m.insert(TermId(i * 7919), i as u32); }
        assert!((0..10_000u64).all(|i| m[&TermId(i * 7919)] == i as u32));
    }
}
