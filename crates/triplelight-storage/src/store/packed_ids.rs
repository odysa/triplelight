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

    /// Replaces `out` with block `blk`'s values, decoded sequentially (one pass over its words).
    pub fn decode_block(&self, blk: usize, out: &mut Vec<u64>) {
        out.clear();
        let n = (self.len - blk * BLOCK).min(BLOCK);
        let (base, w) = (self.base[blk], self.width[blk] as usize);
        if w == 0 { out.resize(n, base); return }
        let words = &self.words[self.start[blk] as usize..];
        let mask = if w == 64 { u64::MAX } else { (1u64 << w) - 1 };
        let mut bit = 0usize;
        out.extend((0..n).map(|_| {
            let (at, sh) = (bit / 64, bit % 64);
            let lo = words[at] >> sh;
            let v = if sh + w > 64 { lo | (words[at + 1] << (64 - sh)) } else { lo };
            bit += w;
            base + (v & mask)
        }));
    }

    /// Appends the encoding to `out` (little-endian): the length, the block count, the four arrays as they are held
    /// (`base`, `width`, `start`), the word count and the words. `read` reverses it without decoding anything.
    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.len as u64).to_le_bytes());
        out.extend_from_slice(&(self.base.len() as u64).to_le_bytes());
        for b in &self.base { out.extend_from_slice(&b.to_le_bytes()) }
        out.extend_from_slice(&self.width);
        for s in &self.start { out.extend_from_slice(&s.to_le_bytes()) }
        out.extend_from_slice(&(self.words.len() as u64).to_le_bytes());
        for w in &self.words { out.extend_from_slice(&w.to_le_bytes()) }
    }

    /// Reads one `write` encoding from the front of `input` and advances it. Refuses an encoding whose arrays do not
    /// fit together (block count against length, widths over 64, a block's codes running past the words), so a
    /// file that passed its checksum but came from a faulty writer cannot make `get` or a scan read out of bounds.
    pub fn read(input: &mut &[u8]) -> std::io::Result<PackedIds> {
        let bad = |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("packed column: {what}"));
        let len = usize::try_from(take_u64(input)?).map_err(|_| bad("length"))?;
        let blocks = usize::try_from(take_u64(input)?).map_err(|_| bad("block count"))?;
        if blocks != len.div_ceil(BLOCK) { return Err(bad("block count does not match the length")) }
        let base = (0..blocks).map(|_| take_u64(input)).collect::<std::io::Result<Vec<u64>>>()?;
        let width = take(input, blocks)?.to_vec();
        let start = (0..blocks).map(|_| take(input, 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))).collect::<std::io::Result<Vec<u32>>>()?;
        let words_len = usize::try_from(take_u64(input)?).map_err(|_| bad("word count"))?;
        if words_len > input.len() / 8 { return Err(bad("word count past the end")) }
        let words = (0..words_len).map(|_| take_u64(input)).collect::<std::io::Result<Vec<u64>>>()?;
        for b in 0..blocks {
            let w = width[b] as usize;
            if w > 64 { return Err(bad("width over 64")) }
            let n = (len - b * BLOCK).min(BLOCK);
            let need = (n * w).div_ceil(64);
            if start[b] as usize + need > words.len() || (b > 0 && start[b] < start[b - 1]) { return Err(bad("block runs past its words")) }
        }
        Ok(PackedIds { len, base, width, start, words })
    }

    /// Keeps in `sel` (positions relative to block `blk`'s first value, ascending) those whose value is in
    /// `lo..hi`. The block's base and width settle blocks wholly inside or outside without decoding.
    pub fn select_in_range(&self, blk: usize, lo: u64, hi: u64, scratch: &mut Vec<u64>, sel: &mut Vec<u32>) {
        let base = self.base[blk];
        if hi <= base { sel.clear(); return }
        let clo = lo.saturating_sub(base);
        let w = self.width[blk];
        let max_code = if w >= 64 { u64::MAX } else { (1u64 << w) - 1 };
        if clo == 0 && hi - base > max_code { return } // the whole block is inside
        if clo > max_code { sel.clear(); return }
        // partly inside: decode the block once, then compare (`scratch` is overwritten)
        self.decode_block(blk, scratch);
        sel.retain(|&r| { let v = scratch[r as usize]; v >= lo && v < hi });
    }
}

/// The first `n` bytes of `input`, advancing it.
pub(super) fn take<'a>(input: &mut &'a [u8], n: usize) -> std::io::Result<&'a [u8]> {
    if input.len() < n { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "table file ends early")) }
    let (head, rest) = input.split_at(n);
    *input = rest;
    Ok(head)
}

/// A little-endian `u64` from the front of `input`, advancing it.
pub(super) fn take_u64(input: &mut &[u8]) -> std::io::Result<u64> { take(input, 8).map(|b| u64::from_le_bytes(b.try_into().unwrap())) }

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
            let mut bytes = Vec::new();
            p.write(&mut bytes);
            let mut input = bytes.as_slice();
            let q = PackedIds::read(&mut input).unwrap();
            assert!(input.is_empty(), "read consumes exactly its encoding");
            for (i, &e) in v.iter().enumerate() { assert_eq!(q.get(i), e) }
            if !bytes.is_empty() { assert!(PackedIds::read(&mut &bytes[..bytes.len() - 1]).is_err(), "a truncated encoding is refused") }
            let (mut d, mut blk) = (Vec::new(), Vec::new());
            for b in 0..v.len().div_ceil(BLOCK) { p.decode_block(b, &mut blk); d.extend_from_slice(&blk) }
            assert_eq!(&d, v);
            for _ in 0..20 {
                let (a, b) = { let (a, b) = (rnd(), rnd()); if v.is_empty() { (a.min(b), a.max(b)) } else { let (a, b) = (v[(a % v.len() as u64) as usize], v[(b % v.len() as u64) as usize]); (a.min(b), a.max(b).saturating_add(rnd() % 2)) } };
                for blk in 0..v.len().div_ceil(BLOCK) {
                    let n = (v.len() - blk * BLOCK).min(BLOCK);
                    let mut sel: Vec<u32> = (0..n as u32).collect();
                    p.select_in_range(blk, a, b, &mut Vec::new(), &mut sel);
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
