//! `BlockView`: seeks, counts and decodes inside one encoded index block without decoding all of it (#143/#144).
//!
//! This is the legacy reader, over today's block encoding (`block_codec`, `RDFIDX03`-`RDFIDX05`): the first column is
//! a running delta and the others are frame-of-reference coded, so a row's bits sit at `row * width` in each column.
//! `Dir::parse` (once, when a block enters a cache) records each 128-row miniblock's first row, after which any one
//! miniblock decodes on its own: column 0 resumes its running sum from that first row, the other columns read their
//! rows directly. A seek is then a search of at most two first rows plus one miniblock decode.
//!
//! The contract agreed in #143: the entry holds `(bytes, Dir)` so building a view is a borrow; `Mini` (one decoded
//! miniblock) is owned by the caller (cursor or per-thread memo), tagged with the block it came from, and a
//! `seek_from` that lands inside the kept miniblock decodes nothing.
use crate::model::TermId;
use std::io::{self, ErrorKind};
use std::ops::Range;

/// Rows per miniblock: the most a `Mini` holds.
pub(crate) const MINI_ROWS: usize = 128;
/// Rows per block this reader accepts (two miniblocks; `Pos` rows fit a `u16`).
const MAX_ROWS: usize = 2 * MINI_ROWS;
const ROTATED: u8 = 0x80;
const TAG_BITS: u32 = 4;

fn invalid(message: &'static str) -> io::Error { io::Error::new(ErrorKind::InvalidData, message) }

/// What `parse` learns about one block's encoding, kept beside its bytes in the cache entry.
#[derive(Clone, Debug)]
pub(crate) struct Dir<const N: usize> {
    rows: usize,
    base: [u64; N],
    width: [u8; N],
    rotated: [bool; N],
    /// Byte offset of each column's packed values within the block.
    col_start: [usize; N],
    /// The first row of each miniblock, and the block's last row.
    firsts: Vec<[TermId; N]>,
    last: [TermId; N],
}

impl<const N: usize> Dir<N> {
    /// Parses and fully checks a block of `rows` rows (`rotate`: the file's format allows rotated columns). The block's
    /// checksum must already have been verified; this decodes it once, as a miss does today, to find each miniblock's
    /// first row.
    pub(crate) fn parse(bytes: &[u8], rows: usize, rotate: bool) -> io::Result<Self> {
        if rows == 0 || rows > MAX_ROWS { return Err(invalid("block view: block longer than two miniblocks")) }
        let all = super::decode::<N>(bytes, rows, rotate)?;
        let base: [u64; N] = std::array::from_fn(|i| u64::from_le_bytes(bytes[i * 8..i * 8 + 8].try_into().unwrap()));
        let flags: [u8; N] = std::array::from_fn(|i| bytes[N * 8 + i]);
        let width = flags.map(|f| f & !ROTATED);
        let rotated = flags.map(|f| f & ROTATED != 0);
        let mut col_start = [0usize; N];
        let mut at = super::header_len(N);
        for c in 0..N { col_start[c] = at; at += (rows * width[c] as usize).div_ceil(8) }
        let firsts = all.iter().step_by(MINI_ROWS).copied().collect();
        Ok(Dir { rows, base, width, rotated, col_start, firsts, last: all[rows - 1] })
    }
    /// Heap bytes this directory adds to its cache entry (for the budget).
    pub(crate) fn heap_bytes(&self) -> usize { self.firsts.capacity() * std::mem::size_of::<[TermId; N]>() }
}

/// A position in a block: the row (block-relative, `0..=len`) and the miniblock the search decoded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Pos { pub mini: u16, pub row: u16 }

/// Which block a `Mini` came from: (index instance, cache epoch, block id). Two views of the same block share it.
pub(crate) type BlockTag = (u64, u64, u64);

/// One decoded miniblock, owned by the caller and reused across calls.
pub(crate) struct Mini<const N: usize> {
    tag: Option<(BlockTag, u16)>,
    len: usize,
    rows: [[TermId; N]; MINI_ROWS],
}

impl<const N: usize> Default for Mini<N> {
    fn default() -> Self { Mini { tag: None, len: 0, rows: [[TermId(0); N]; MINI_ROWS] } }
}

impl<const N: usize> Mini<N> {
    /// The decoded rows.
    pub(crate) fn rows(&self) -> &[[TermId; N]] { &self.rows[..self.len] }
}

/// A borrowed view of one encoded block.
pub(crate) struct BlockView<'a, const N: usize> {
    bytes: &'a [u8],
    dir: &'a Dir<N>,
    tag: BlockTag,
}

impl<'a, const N: usize> BlockView<'a, N> {
    pub(crate) fn new(bytes: &'a [u8], dir: &'a Dir<N>, tag: BlockTag) -> Self { BlockView { bytes, dir, tag } }
    pub(crate) fn len(&self) -> usize { self.dir.rows }
    pub(crate) fn first(&self) -> [TermId; N] { self.dir.firsts[0] }
    pub(crate) fn last(&self) -> [TermId; N] { self.dir.last }
    fn minis(&self) -> usize { self.dir.firsts.len() }
    fn mini_range(&self, m: usize) -> Range<usize> { m * MINI_ROWS..((m + 1) * MINI_ROWS).min(self.dir.rows) }

    /// Decodes miniblock `m` into `mini`, unless it already holds it.
    fn load(&self, m: usize, mini: &mut Mini<N>) {
        let want = (self.tag, m as u16);
        if mini.tag == Some(want) {
            debug_assert_eq!(mini.rows[0], self.dir.firsts[m], "a Mini tagged for this block holds another block's rows");
            return;
        }
        let range = self.mini_range(m);
        let d = self.dir;
        for c in 0..N {
            let w = d.width[c] as usize;
            // Stream the column's bits from this miniblock's first row, LSB-first as `block_codec` packs them, reading
            // each byte once.
            let col = &self.bytes[d.col_start[c]..];
            let bit = range.start * w;
            let (mut at, mut acc, mut have) = (bit / 8, 0u128, 0usize);
            if w > 0 && bit % 8 != 0 { acc = (col[at] >> (bit % 8)) as u128; have = 8 - bit % 8; at += 1 }
            let mask = if w == 0 { 0 } else { u128::MAX >> (128 - w) };
            let mut next = || -> u64 {
                if w == 0 { return 0 }
                while have < w { acc |= (col[at] as u128) << have; at += 1; have += 8 }
                let v = (acc & mask) as u64;
                acc >>= w;
                have -= w;
                v
            };
            if c == 0 {
                let mut v = d.firsts[m][0].0;
                next(); // this row's own delta: its value is the miniblock's recorded first row
                mini.rows[0][0] = TermId(v);
                for row in mini.rows[1..range.len()].iter_mut() { v += next(); row[0] = TermId(v) }
            } else {
                for row in mini.rows[..range.len()].iter_mut() {
                    let v = d.base[c] + next();
                    row[c] = TermId(if d.rotated[c] { v.rotate_right(TAG_BITS) } else { v });
                }
            }
        }
        mini.len = range.len();
        mini.tag = Some(want);
        debug_assert_eq!(mini.rows[0], d.firsts[m]);
    }

    /// The first row `>= key` (`> key` when `upper`): the last miniblock starting before it, decoded, searched.
    fn bound(&self, key: &[TermId; N], upper: bool, mini: &mut Mini<N>) -> Pos {
        let before = |r: &[TermId; N]| r < key || (upper && r == key);
        // The miniblock to search: the last one whose first row is `<= key` (a key equal to a miniblock's first row
        // lands in that miniblock, so the returned position always lies in the decoded one, or at its end).
        let m = self.dir.firsts.partition_point(|f| f <= key).saturating_sub(1);
        self.load(m, mini);
        let j = mini.rows().partition_point(|r| before(r));
        Pos { mini: m as u16, row: (m * MINI_ROWS + j) as u16 }
    }

    /// The first row `>= key`.
    pub(crate) fn seek(&self, key: &[TermId; N], mini: &mut Mini<N>) -> Pos { self.bound(key, false, mini) }

    /// As `seek`, resuming at `hint`: when `mini` already holds `hint`'s miniblock of this block and the key falls
    /// inside it, a search of the kept rows with no decode. Anything else (another block, a key past the kept
    /// miniblock or before it) is a full `seek`.
    pub(crate) fn seek_from(&self, hint: Pos, key: &[TermId; N], mini: &mut Mini<N>) -> Pos {
        if mini.tag == Some((self.tag, hint.mini)) {
            let rows = mini.rows();
            if rows.first().is_some_and(|f| f <= key) && rows.last().is_some_and(|l| key <= l) {
                let j = rows.partition_point(|r| r < key);
                return Pos { mini: hint.mini, row: (hint.mini as usize * MINI_ROWS + j) as u16 };
            }
        }
        self.seek(key, mini)
    }

    /// Rows whose first `prefix.len()` IDs equal `prefix` (two seeks).
    pub(crate) fn range(&self, prefix: &[TermId], mini: &mut Mini<N>) -> Range<usize> {
        debug_assert!(prefix.len() <= N);
        let (mut low, mut high) = ([TermId(0); N], [TermId(u64::MAX); N]);
        low[..prefix.len()].copy_from_slice(prefix);
        high[..prefix.len()].copy_from_slice(prefix);
        let a = self.bound(&low, false, mini).row as usize;
        let b = self.bound(&high, true, mini).row as usize;
        a..b
    }

    pub(crate) fn count(&self, prefix: &[TermId], mini: &mut Mini<N>) -> usize { self.range(prefix, mini).len() }

    /// Appends rows `rows` of this block to `out`, decoding only the miniblocks they cover.
    pub(crate) fn decode(&self, rows: Range<usize>, out: &mut Vec<[TermId; N]>, mini: &mut Mini<N>) {
        let rows = rows.start..rows.end.min(self.len());
        if rows.is_empty() { return }
        for m in rows.start / MINI_ROWS..=(rows.end - 1) / MINI_ROWS {
            self.load(m, mini);
            let r = self.mini_range(m);
            let (from, to) = (rows.start.max(r.start) - r.start, rows.end.min(r.end) - r.start);
            out.extend_from_slice(&mini.rows()[from..to]);
        }
        debug_assert!(self.minis() <= 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng(state: &mut u64) -> u64 { *state ^= *state << 13; *state ^= *state >> 7; *state ^= *state << 17; *state }

    /// Sorted unique rows with runs in the first two columns and mixed term tags in the last, like an SPO block.
    fn block(len: usize, seed: u64) -> Vec<[TermId; 3]> {
        let mut s = seed | 1;
        let mut rows: Vec<[TermId; 3]> = Vec::with_capacity(len);
        let (mut a, mut b) = (1 + rng(&mut s) % 1000, 1u64);
        while rows.len() < len {
            match rng(&mut s) % 8 { 0 => { a += 1 + rng(&mut s) % 50; b = 1 }, 1 | 2 => b += 1 + rng(&mut s) % 3, _ => {} }
            let tag = [1u64, 3, 0xf][(rng(&mut s) % 3) as usize];
            let c = TermId((tag << 60) | (1 + rng(&mut s) % 1_000_000));
            let row = [TermId(a), TermId(b), c];
            if rows.last().is_none_or(|l| *l < row) { rows.push(row) } else { b += 1 }
        }
        rows
    }

    #[test]
    fn seeks_counts_and_decodes_agree_with_the_decoded_block() {
        let mut s = 0x2545_f491_4f6c_dd1du64;
        for len in [1usize, 2, 127, 128, 129, 200, 255, 256] {
            for rotate in [false, true] {
                let rows = block(len, rng(&mut s));
                let bytes = super::super::encode(&rows, rotate).unwrap();
                let dir = Dir::<3>::parse(&bytes, len, rotate).unwrap();
                let view = BlockView::new(&bytes, &dir, (1, 0, 7));
                assert_eq!((view.len(), view.first(), view.last()), (len, rows[0], rows[len - 1]));
                let mut mini = Mini::default();
                // Every row, and keys just below and above it.
                for r in &rows {
                    for key in [*r, [r[0], r[1], TermId(r[2].0.wrapping_sub(1))], [r[0], r[1], TermId(r[2].0 + 1)]] {
                        let want = rows.partition_point(|x| *x < key);
                        assert_eq!(view.seek(&key, &mut mini).row as usize, want, "len {len} rotate {rotate} {key:?}");
                    }
                    for prefix in [&r[..1], &r[..2], &r[..]] {
                        let want = rows.iter().filter(|x| x[..prefix.len()] == *prefix).count();
                        assert_eq!(view.count(prefix, &mut mini), want, "count {prefix:?}");
                    }
                }
                for key in [[TermId(0); 3], [TermId(u64::MAX); 3]] {
                    assert_eq!(view.seek(&key, &mut mini).row as usize, rows.partition_point(|x| *x < key));
                }
                // Any sub-range decodes to the same rows.
                for _ in 0..50 {
                    let a = (rng(&mut s) as usize) % (len + 1);
                    let b = a + (rng(&mut s) as usize) % (len + 1 - a);
                    let mut out = Vec::new();
                    view.decode(a..b, &mut out, &mut mini);
                    assert_eq!(out, rows[a..b], "decode {a}..{b} of {len}");
                }
            }
        }
    }

    #[test]
    fn seek_from_reuses_the_kept_miniblock_and_falls_back_otherwise() {
        let rows = block(256, 99);
        let bytes = super::super::encode(&rows, true).unwrap();
        let dir = Dir::<3>::parse(&bytes, 256, true).unwrap();
        let view = BlockView::new(&bytes, &dir, (1, 0, 3));
        let mut mini = Mini::default();
        let mut pos = view.seek(&rows[10], &mut mini);
        let mut s = 7u64;
        let mut i = 10usize;
        for _ in 0..2000 {
            // Mostly ascending keys, sometimes a jump back (the #349 cursor's occasional backward probe).
            i = if rng(&mut s) % 20 == 0 { (rng(&mut s) as usize) % 256 } else { (i + (rng(&mut s) as usize) % 4) % 256 };
            pos = view.seek_from(pos, &rows[i], &mut mini);
            assert_eq!(pos.row as usize, i);
            assert_eq!(mini.rows()[i - pos.mini as usize * MINI_ROWS], rows[i]);
        }
        // A Mini kept from another block is never trusted.
        let other = block(256, 5);
        let other_bytes = super::super::encode(&other, true).unwrap();
        let other_dir = Dir::<3>::parse(&other_bytes, 256, true).unwrap();
        let other_view = BlockView::new(&other_bytes, &other_dir, (1, 0, 4));
        let p = other_view.seek_from(pos, &other[200], &mut mini);
        assert_eq!(p.row, 200);
    }

    #[test]
    fn parse_refuses_a_block_longer_than_two_miniblocks() {
        let rows = block(256, 3);
        let bytes = super::super::encode(&rows, true).unwrap();
        assert!(Dir::<3>::parse(&bytes, 0, true).is_err());
        assert!(Dir::<3>::parse(&bytes, 257, true).is_err());
        assert!(Dir::<3>::parse(&bytes[..bytes.len() - 1], 256, true).is_err(), "a truncated block");
    }
}
