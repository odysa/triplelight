//! Internal codec for one block of sorted triple IDs, the per-block checksum (`crc32c`) and the in-place reader
//! over an encoded block (`view`).

// The encoded-block reader #144's two-tier cache is built on; only its own tests use it until that lands.
#[allow(dead_code)]
pub(crate) mod view;
use crate::model::TermId;
use std::io::{self, ErrorKind};

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message)
}

/// Bytes before the packed columns of an `N`-column block: `N` bases and `N` bit widths.
pub(super) const fn header_len(n: usize) -> usize { n * 9 }

/// Width-byte flag (`RDFIDX04`/`RDFQIX02` and later): the column's values are stored as `id.rotate_left(TAG_BITS)`,
/// which moves the 4-bit term tag (`id >> 60`) to the bottom. A block whose column mixes term kinds (an SPO object
/// column holding IRIs, literals and inline numbers) then spans the kinds' rank ranges instead of about 2^62. Widths
/// are at most 64, so the top bit of the width byte is free; older formats reject it.
const ROTATED: u8 = 0x80;
const TAG_BITS: u32 = 4;

fn bit_width(span: u64) -> u8 { (64 - span.leading_zeros()) as u8 }

/// `N` 64-bit bases, `N` bit widths, then `N` tightly packed columns (`N` = 3 for triples, 4 for quads; the 3-column
/// layout is unchanged). First column is delta-coded from the preceding row; the others use block minima, of the IDs
/// or, if `rotate` allows and it is narrower, of the tag-rotated IDs (flag `ROTATED` in that column's width byte).
pub(super) fn encode<const N: usize>(rows: &[[TermId; N]], rotate: bool) -> io::Result<Vec<u8>> {
    if rows.is_empty() || rows.len() > 256 {
        return Err(invalid("invalid block row count"));
    }
    // Per column after the first: rotate when that gives a narrower span (ties keep the plain IDs).
    let rotated: [bool; N] = std::array::from_fn(|c| {
        c > 0 && rotate && {
            let span = |f: &dyn Fn(u64) -> u64| {
                let (lo, hi) = rows.iter().fold((u64::MAX, 0), |(l, h), r| (l.min(f(r[c].0)), h.max(f(r[c].0))));
                hi - lo
            };
            bit_width(span(&|v| v.rotate_left(TAG_BITS))) < bit_width(span(&|v| v))
        }
    });
    let stored = |c: usize, v: u64| if rotated[c] { v.rotate_left(TAG_BITS) } else { v };
    let base: [u64; N] = std::array::from_fn(|c| if c == 0 { rows[0][0].0 } else { rows.iter().map(|r| stored(c, r[c].0)).min().unwrap() });
    let mut values: [Vec<u64>; N] = std::array::from_fn(|_| Vec::with_capacity(rows.len()));
    let mut prev = base[0];
    for row in rows {
        let first = row[0]
            .0
            .checked_sub(prev)
            .ok_or_else(|| invalid("unsorted first column"))?;
        values[0].push(first);
        prev = row[0].0;
        for c in 1..N {
            values[c].push(stored(c, row[c].0) - base[c]);
        }
    }
    let widths = values
        .each_ref()
        .map(|v| (64 - v.iter().copied().max().unwrap_or(0).leading_zeros()) as u8);
    let mut out = Vec::with_capacity(header_len(N) + rows.len() * N * 8);
    for n in base {
        out.extend_from_slice(&n.to_le_bytes());
    }
    out.extend(widths.iter().zip(rotated).map(|(w, r)| if r { w | ROTATED } else { *w }));
    for col in 0..N {
        let width = widths[col] as usize;
        let mut acc = 0u128;
        let mut bits = 0usize;
        for &value in &values[col] {
            acc |= (value as u128) << bits;
            bits += width;
            while bits >= 8 {
                out.push(acc as u8);
                acc >>= 8;
                bits -= 8;
            }
        }
        if bits != 0 {
            out.push(acc as u8);
        }
    }
    Ok(out)
}

/// `rotate`: the file's format allows tag-rotated columns (`RDFIDX04`/`RDFQIX02`); older blocks never set the flag.
pub(super) fn decode<const N: usize>(data: &[u8], count: usize, rotate: bool) -> io::Result<Vec<[TermId; N]>> {
    if count == 0 || count > 256 || data.len() < header_len(N) {
        return Err(invalid("invalid compressed block"));
    }
    let base: [u64; N] = std::array::from_fn(|i| u64::from_le_bytes(data[i * 8..i * 8 + 8].try_into().unwrap()));
    let flags: [u8; N] = std::array::from_fn(|i| data[N * 8 + i]);
    let rotated: [bool; N] = std::array::from_fn(|i| flags[i] & ROTATED != 0);
    let widths: [u8; N] = std::array::from_fn(|i| flags[i] & !ROTATED);
    if widths.iter().any(|w| *w > 64) || (rotated.iter().any(|r| *r) && !rotate) || rotated[0] {
        return Err(invalid("invalid column bit width"));
    }
    let mut rows = vec![[TermId(0); N]; count];
    let mut cursor = header_len(N);
    for col in 0..N {
        let width = widths[col] as usize;
        let len = (count * width).div_ceil(8);
        let end = cursor
            .checked_add(len)
            .filter(|&v| v <= data.len())
            .ok_or_else(|| invalid("truncated compressed column"))?;
        let slice = &data[cursor..end];
        let (mut acc, mut bits, mut at) = (0u128, 0usize, 0usize);
        let mask = (1u128 << width) - 1; // width <= 64, checked above
        let mut last = base[col];
        for row in &mut rows {
            while bits < width {
                acc |= (slice[at] as u128) << bits;
                bits += 8;
                at += 1;
            }
            let delta = (acc & mask) as u64;
            acc >>= width;
            bits -= width;
            let value = if col == 0 {
                last.checked_add(delta)
            } else {
                base[col].checked_add(delta)
            }
            .ok_or_else(|| invalid("compressed ID overflow"))?;
            row[col] = TermId(value);
            last = value;
        }
        if acc != 0 {
            return Err(invalid("nonzero compressed padding"));
        }
        // Undo the tag rotation in a separate pass, so the unpacking loop stays branch-free for every column.
        if rotated[col] {
            for row in &mut rows { row[col] = TermId(row[col].0.rotate_right(TAG_BITS)) }
        }
        cursor = end;
    }
    if cursor != data.len() {
        return Err(invalid("compressed block trailing bytes"));
    }
    Ok(rows)
}

/// CRC-32C (Castagnoli), the per-block checksum of `RDFIDX05`/`RDFQIX03` index files: 0.2-1.2 ns per row in
/// hardware against SHA-256's 7-20 ns per row, which cost more than decoding the block (#143). It detects accidental
/// corruption (bit rot, torn writes, bad reads; every burst of up to 32 bits). A file's identity, the SHA-256 of its
/// header and directory, then binds block contents via CRC-32C: it detects accidental change, not deliberate change.
/// The files are not authenticated in any format.
pub(crate) mod crc32c {
    /// CRC-32C of `data` (initial value and final XOR `0xffff_ffff`, reflected, as iSCSI and ext4).
    pub(crate) fn crc32c(data: &[u8]) -> u32 {
        #[cfg(target_arch = "aarch64")]
        if std::arch::is_aarch64_feature_detected!("crc") {
            // SAFETY: the `crc` feature was detected at run time.
            return unsafe { hw_aarch64(data) };
        }
        #[cfg(target_arch = "x86_64")]
        if std::arch::is_x86_feature_detected!("sse4.2") {
            // SAFETY: `sse4.2` was detected at run time.
            return unsafe { hw_x86(data) };
        }
        software(data)
    }

    #[cfg(target_arch = "aarch64")]
    #[target_feature(enable = "crc")]
    unsafe fn hw_aarch64(data: &[u8]) -> u32 {
        use std::arch::aarch64::{__crc32cb, __crc32cd};
        let mut crc = !0u32;
        let (words, rest) = data.as_chunks::<8>();
        for w in words { crc = __crc32cd(crc, u64::from_le_bytes(*w)) }
        for &b in rest { crc = __crc32cb(crc, b) }
        !crc
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "sse4.2")]
    unsafe fn hw_x86(data: &[u8]) -> u32 {
        use std::arch::x86_64::{_mm_crc32_u64, _mm_crc32_u8};
        let mut crc = !0u32;
        let (words, rest) = data.as_chunks::<8>();
        for w in words { crc = _mm_crc32_u64(crc as u64, u64::from_le_bytes(*w)) as u32 }
        for &b in rest { crc = _mm_crc32_u8(crc, b) }
        !crc
    }

    /// Bitwise fallback for CPUs without the instruction (only the wasm/other targets, which have no disk stores).
    fn software(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 { crc = (crc >> 1) ^ (0x82f6_3b78 & (crc & 1).wrapping_neg()) }
        }
        !crc
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn known_vectors_and_the_hardware_path_agrees_with_software() {
            // RFC 3720 (iSCSI) B.4 test vectors.
            assert_eq!(crc32c(b"123456789"), 0xe306_9283);
            assert_eq!(crc32c(&[0u8; 32]), 0x8a91_36aa);
            assert_eq!(crc32c(&[0xffu8; 32]), 0x62a8_ab43);
            assert_eq!(crc32c(&(0..32u8).collect::<Vec<_>>()), 0x46dd_794e);
            assert_eq!(crc32c(b""), 0);
            let data: Vec<u8> = (0..1000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
            for len in [0, 1, 7, 8, 9, 63, 64, 65, 999, 1000] { assert_eq!(crc32c(&data[..len]), software(&data[..len]), "len {len}") }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diverse_lengths_and_cross_byte_fields_round_trip() {
        let mut rng = 0x9e3779b97f4a7c15u64;
        for count in [1, 2, 7, 31, 255, 256] {
            let mut first = 1u64;
            let mut rows = Vec::new();
            for _ in 0..count {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                first += rng & 31;
                rows.push([TermId(first), TermId((rng & 0xffff) + 1), TermId(rng | 1)]);
            }
            let data = encode(&rows, true).unwrap();
            assert_eq!(decode::<3>(&data, count, true).unwrap(), rows);
            for cut in [0, 26, data.len() - 1] {
                assert!(decode::<3>(&data[..cut], count, true).is_err());
            }
        }
    }
    #[test]
    fn four_columns_round_trip_and_three_columns_are_unchanged() {
        let mut rng = 0x2545f4914f6cdd1du64;
        for count in [1, 3, 255, 256] {
            let mut first = 1u64;
            let rows: Vec<[TermId; 4]> = (0..count).map(|_| {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                first += rng & 3;
                [TermId(first), TermId((rng & 0xff) + 1), TermId(rng | 1), TermId((rng >> 3) | 1)]
            }).collect();
            let data = encode(&rows, true).unwrap();
            assert_eq!(data[..8], first_base(&rows));
            assert_eq!(decode::<4>(&data, count, true).unwrap(), rows);
            assert!(decode::<4>(&data[..header_len(4) - 1], count, true).is_err());
            assert!(decode::<4>(&data[..data.len() - 1], count, true).is_err());
        }
        // The triple layout is byte-for-byte the pre-generic one: bases, widths, then packed columns.
        let rows = [[TermId(3), TermId(5), TermId(9)], [TermId(4), TermId(6), TermId(9)]];
        assert_eq!(encode(&rows, true).unwrap(), [
            3, 0, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, // bases
            1, 1, 0, // widths: first-column deltas 0,1; second column 0,1; third column constant
            0b10, 0b10, // packed columns 0 and 1 (two 1-bit values each); column 2 has width 0
        ]);
    }
    fn first_base<const N: usize>(rows: &[[TermId; N]]) -> [u8; 8] { rows[0][0].0.to_le_bytes() }
    #[test]
    fn extremes_and_zero_width_round_trip() {
        let rows = vec![
            [TermId(1), TermId(7), TermId(u64::MAX)],
            [TermId(1), TermId(8), TermId(1)],
            [TermId(u64::MAX), TermId(u64::MAX), TermId(9)],
        ];
        let bytes = encode(&rows, true).unwrap();
        assert_eq!(decode::<3>(&bytes, rows.len(), true).unwrap(), rows);
        let same = vec![[TermId(5), TermId(6), TermId(7)]; 256];
        assert_eq!(decode::<3>(&encode(&same, true).unwrap(), 256, true).unwrap(), same);
        assert!(decode::<3>(&bytes[..26], rows.len(), true).is_err());
        let mut bad = bytes.clone();
        bad[24] = 65;
        assert!(decode::<3>(&bad, rows.len(), true).is_err());
        let mut bad = bytes;
        bad.push(0);
        assert!(decode::<3>(&bad, rows.len(), true).is_err());
    }
    #[test]
    fn mixed_tag_columns_rotate_and_older_formats_reject_the_flag() {
        // An SPO-like block: subjects ascend, predicates are IRIs, objects mix IRIs (tag 1), literals (tag 3) and
        // inline numbers (tag 0xf).
        let tag = |t: u64, rank: u64| TermId((t << 60) | rank);
        let rows: Vec<[TermId; 3]> = (0..256u64)
            .map(|i| [tag(1, 1000 + i / 8), tag(1, 7 + i % 8), tag([1, 3, 0xf][(i % 3) as usize], 50_000 + i * 13)])
            .collect();
        let data = encode(&rows, true).unwrap();
        assert_eq!(data[24 + 2] & ROTATED, ROTATED, "the object column is rotated");
        assert_eq!(data[24 + 1] & ROTATED, 0, "a single-tag column is not");
        assert!((data[24 + 2] & !ROTATED) < 32, "rotation narrows the column from about 62 bits");
        assert_eq!(decode::<3>(&data, 256, true).unwrap(), rows);
        assert!(decode::<3>(&data, 256, false).is_err(), "RDFIDX03 readers reject a rotated block");
        let plain = encode(&rows, false).unwrap();
        assert!(plain.len() > data.len() + 256 * 3, "the plain encoding needs about 62 bits per object");
        assert_eq!(decode::<3>(&plain, 256, false).unwrap(), rows);
        assert_eq!(decode::<3>(&plain, 256, true).unwrap(), rows, "new readers read old blocks");
        let mut bad = data.clone();
        bad[24] |= ROTATED;
        assert!(decode::<3>(&bad, 256, true).is_err(), "the delta-coded first column is never rotated");
    }
}
