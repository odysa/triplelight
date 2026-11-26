//! Frame-of-reference bit-packed `u64` arrays for wide-table columns (#140).
//!
//! Values are cut into blocks of `BLOCK` (1024, the scan batch size). A block stores its minimum and the bit width
//! of `value - minimum`; codes are packed little-endian into `u64` words and may straddle a word boundary. Sorted
//! columns (subjects, offsets) and columns of IDs of one kind pack to a few bits per value. A range test
//! `lo <= v < hi` runs on the codes of a block without decoding it (`select_in_range`).

/// Values per block; equal to the table scan's batch size so a dense batch is exactly one block.
pub const BLOCK: usize = 1024;

#[derive(Debug, Clone, Default)]
pub struct PackedIds {
    len: usize,
    /// Per block: minimum value.
    base: Vec<u64>,
    /// Per block: bits per code (0..=64).
    width: Vec<u8>,
    /// Per block: index of its first word in `words`.
    start: Vec<u32>,
    words: Vec<u64>,
}

impl PackedIds {
    pub fn from_slice(values: &[u64]) -> Self {
        let mut p = PackedIds { len: values.len(), ..Default::default() };
        for chunk in values.chunks(BLOCK) {
            let (min, max) = chunk.iter().fold((u64::MAX, 0), |(lo, hi), &v| (lo.min(v), hi.max(v)));
            let w = 64 - (max - min).leading_zeros();
            p.base.push(min);
            p.width.push(w as u8);
            p.start.push(u32::try_from(p.words.len()).expect("packed column over 2^32 words"));
            if w == 0 { continue }
            let mut acc: u128 = 0;
            let mut bits = 0u32;
            for &v in chunk {
                acc |= ((v - min) as u128) << bits;
                bits += w;
                while bits >= 64 { p.words.push(acc as u64); acc >>= 64; bits -= 64 }
            }
            if bits > 0 { p.words.push(acc as u64) }
        }
        p
    }

    pub fn len(&self) -> usize { self.len }
    pub fn is_empty(&self) -> bool { self.len == 0 }

    /// Heap bytes of the encoding.
    pub fn bytes(&self) -> u64 { (self.base.len() * 8 + self.width.len() + self.start.len() * 4 + self.words.len() * 8) as u64 }

    #[inline]
    fn code(&self, b: usize, j: usize) -> u64 {
        let w = self.width[b] as usize;
        if w == 0 { return 0 }
        let bit = j * w;
        let at = self.start[b] as usize + bit / 64;
        let shift = bit % 64;
        let lo = self.words[at] >> shift;
        let v = if shift + w > 64 { lo | (self.words[at + 1] << (64 - shift)) } else { lo };
        if w == 64 { v } else { v & ((1u64 << w) - 1) }
    }

    #[inline]
    pub fn get(&self, i: usize) -> u64 {
        debug_assert!(i < self.len);
        let (b, j) = (i / BLOCK, i % BLOCK);
        self.base[b] + self.code(b, j)
    }

    /// For ascending values: the number of values below `x` (the index of the first value `>= x`).
    pub fn partition_point(&self, x: u64) -> usize {
        // blocks whose base is below x, then a binary search inside the last of them
        let b = self.base.partition_point(|&m| m < x);
        if b == 0 { return 0 }
        let blk = b - 1;
        let n = (self.len - blk * BLOCK).min(BLOCK);
        let (mut lo, mut hi) = (0, n);
        while lo < hi { let mid = (lo + hi) / 2; if self.base[blk] + self.code(blk, mid) < x { lo = mid + 1 } else { hi = mid } }
        blk * BLOCK + lo
    }

    /// Appends `values[a..b]` to `out`.
    pub fn decode_into(&self, a: usize, b: usize, out: &mut Vec<u64>) {
        out.reserve(b.saturating_sub(a));
        for i in a..b { out.push(self.get(i)) }
    }

    /// Keeps in `sel` (positions relative to block `blk`'s first value, ascending) those whose value is in
    /// `lo..hi`, comparing codes against the range shifted by the block's base.
    pub fn select_in_range(&self, blk: usize, lo: u64, hi: u64, sel: &mut Vec<u32>) {
        let base = self.base[blk];
        if hi <= base { sel.clear(); return }
        let clo = lo.saturating_sub(base);
        let chi = hi - base;
        let w = self.width[blk];
        let max_code = if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
        if clo == 0 && chi > max_code { return } // the whole block is inside
        if clo > max_code { sel.clear(); return }
        sel.retain(|&r| { let c = self.code(blk, r as usize); c >= clo && c < chi });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_filters_like_the_plain_array() {
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut rnd = || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x };
        let cases: Vec<Vec<u64>> = vec![
            vec![],
            vec![7; 3000],
            (0..2500).collect(),
            (0..3000).map(|_| rnd()).collect(),
            (0..2049).map(|_| (1u64 << 62) | (rnd() % 100_000)).collect(),
            vec![0, u64::MAX, 5, u64::MAX - 1],
        ];
        for v in &cases {
            let p = PackedIds::from_slice(v);
            assert_eq!(p.len(), v.len());
            for (i, &e) in v.iter().enumerate() { assert_eq!(p.get(i), e) }
            let mut d = Vec::new();
            p.decode_into(0, v.len(), &mut d);
            assert_eq!(&d, v);
            for _ in 0..20 {
                let (a, b) = { let (a, b) = (rnd(), rnd()); if v.is_empty() { (a.min(b), a.max(b)) } else { let (a, b) = (v[(a % v.len() as u64) as usize], v[(b % v.len() as u64) as usize]); (a.min(b), a.max(b).saturating_add(rnd() % 2)) } };
                for blk in 0..v.len().div_ceil(BLOCK) {
                    let n = (v.len() - blk * BLOCK).min(BLOCK);
                    let mut sel: Vec<u32> = (0..n as u32).collect();
                    p.select_in_range(blk, a, b, &mut sel);
                    let want: Vec<u32> = (0..n as u32).filter(|&r| { let e = v[blk * BLOCK + r as usize]; e >= a && e < b }).collect();
                    assert_eq!(sel, want);
                }
            }
        }
        assert!(PackedIds::from_slice(&(0..4096).collect::<Vec<_>>()).bytes() < 4096 * 2, "a sorted run packs to ~10 bits per value");
        let sorted: Vec<u64> = { let mut v: Vec<u64> = (0..3000).map(|_| rnd() % 50_000).collect(); v.sort(); v };
        let p = PackedIds::from_slice(&sorted);
        for x in [0, 1, 25_000, 49_999, 50_000, u64::MAX].into_iter().chain(sorted.iter().copied().step_by(97)) {
            assert_eq!(p.partition_point(x), sorted.partition_point(|&v| v < x), "x={x}");
        }
    }
}
