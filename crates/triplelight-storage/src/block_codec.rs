//! Internal codec for one block of sorted triple IDs.
use crate::model::TermId;
use std::io::{self, ErrorKind};

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, message)
}

/// Three 64-bit bases, three bit widths, then three tightly packed columns.
/// First column is delta-coded from the preceding row; the others use block minima.
pub(super) fn encode(rows: &[[TermId; 3]]) -> io::Result<Vec<u8>> {
    if rows.is_empty() || rows.len() > 256 {
        return Err(invalid("invalid block row count"));
    }
    let base = [
        rows[0][0].0,
        rows.iter().map(|r| r[1].0).min().unwrap(),
        rows.iter().map(|r| r[2].0).min().unwrap(),
    ];
    let mut values = [
        Vec::with_capacity(rows.len()),
        Vec::with_capacity(rows.len()),
        Vec::with_capacity(rows.len()),
    ];
    let mut prev = base[0];
    for row in rows {
        let first = row[0]
            .0
            .checked_sub(prev)
            .ok_or_else(|| invalid("unsorted first column"))?;
        values[0].push(first);
        prev = row[0].0;
        values[1].push(row[1].0 - base[1]);
        values[2].push(row[2].0 - base[2]);
    }
    let widths = values
        .each_ref()
        .map(|v| (64 - v.iter().copied().max().unwrap_or(0).leading_zeros()) as u8);
    let mut out = Vec::with_capacity(27 + rows.len() * 24);
    for n in base {
        out.extend_from_slice(&n.to_le_bytes());
    }
    out.extend_from_slice(&widths);
    for col in 0..3 {
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

pub(super) fn decode(data: &[u8], count: usize) -> io::Result<Vec<[TermId; 3]>> {
    if count == 0 || count > 256 || data.len() < 27 {
        return Err(invalid("invalid compressed block"));
    }
    let mut base = [0u64; 3];
    for i in 0..3 {
        base[i] = u64::from_le_bytes(data[i * 8..i * 8 + 8].try_into().unwrap());
    }
    let widths = [data[24], data[25], data[26]];
    if widths.iter().any(|w| *w > 64) {
        return Err(invalid("invalid column bit width"));
    }
    let mut rows = vec![[TermId(0); 3]; count];
    let mut cursor = 27usize;
    for col in 0..3 {
        let width = widths[col] as usize;
        let len = (count * width).div_ceil(8);
        let end = cursor
            .checked_add(len)
            .filter(|&v| v <= data.len())
            .ok_or_else(|| invalid("truncated compressed column"))?;
        let slice = &data[cursor..end];
        let (mut acc, mut bits, mut at) = (0u128, 0usize, 0usize);
        let mask = if width == 64 {
            u64::MAX as u128
        } else {
            (1u128 << width) - 1
        };
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
        cursor = end;
    }
    if cursor != data.len() {
        return Err(invalid("compressed block trailing bytes"));
    }
    Ok(rows)
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
            let data = encode(&rows).unwrap();
            assert_eq!(decode(&data, count).unwrap(), rows);
            for cut in [0, 26, data.len() - 1] {
                assert!(decode(&data[..cut], count).is_err());
            }
        }
    }
    #[test]
    fn extremes_and_zero_width_round_trip() {
        let rows = vec![
            [TermId(1), TermId(7), TermId(u64::MAX)],
            [TermId(1), TermId(8), TermId(1)],
            [TermId(u64::MAX), TermId(u64::MAX), TermId(9)],
        ];
        let bytes = encode(&rows).unwrap();
        assert_eq!(decode(&bytes, rows.len()).unwrap(), rows);
        let same = vec![[TermId(5), TermId(6), TermId(7)]; 256];
        assert_eq!(decode(&encode(&same).unwrap(), 256).unwrap(), same);
        assert!(decode(&bytes[..26], rows.len()).is_err());
        let mut bad = bytes.clone();
        bad[24] = 65;
        assert!(decode(&bad, rows.len()).is_err());
        let mut bad = bytes;
        bad.push(0);
        assert!(decode(&bad, rows.len()).is_err());
    }
}
