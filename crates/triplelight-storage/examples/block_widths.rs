//! Block bit-width census (#105, storage lane; read-only). Re-derives, from a full scan of each order, the packed
//! column widths `block_codec::encode` gives every 256-row block, and the widths an alternative encoding would give.
//! The alternative moves an ID's 4-bit term tag (`id >> 60`) to the bottom of the value before the frame of
//! reference, for every column except the first (`rotl(id, 4)`). Only the first column is delta-coded, so the others
//! need not be order-preserving inside the encoding. Rotating costs 4 bits on a single-tag column, so the alternative
//! takes, per block and column, the narrower of the two (as an encoder with a per-column flag would).
//!
//!   cargo run --release -p triplelight-storage --example block_widths -- <base-dir>
//!
//! Prints, per order, the mean width of each column under both encodings, the packed payload bytes (headers
//! excluded) under both, and the share of blocks whose column mixes term tags. For the non-first columns it also
//! estimates a block-local dictionary and a per-tag frame of reference (headers included), and the payload if each
//! block and column took the narrowest of the four encodings.
use triplelight_core::model::{Perm, TermId};
use triplelight_storage::{DiskSnapshot, Snapshot};

fn width(max: u64) -> u64 { 64 - max.leading_zeros() as u64 }
fn bits_for(n: usize) -> u64 { if n <= 1 { 0 } else { width(n as u64 - 1) } }

/// Bits for one non-first column of a block under two further candidate encodings (headers included):
/// - a block-local dictionary: the distinct values, frame-of-reference packed, plus one code per row;
/// - frame of reference per tag group: a base per distinct tag, a tag code per row, and each value against its tag's base.
fn dict_and_per_tag_bits(values: &[u64]) -> (u64, u64) {
    let mut distinct: Vec<u64> = values.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    let (lo, hi) = (distinct[0], *distinct.last().unwrap());
    let dict = 64 + 8 + distinct.len() as u64 * width(hi - lo) + values.len() as u64 * bits_for(distinct.len());
    let mut tags: Vec<(u64, u64, u64, u64)> = Vec::new(); // (tag, min, max, count)
    for &v in values {
        let t = v >> 60;
        match tags.iter_mut().find(|e| e.0 == t) {
            Some(e) => { e.1 = e.1.min(v); e.2 = e.2.max(v); e.3 += 1 }
            None => tags.push((t, v, v, 1)),
        }
    }
    let per_tag = tags.iter().map(|e| 64 + 8 + e.3 * width(e.2 - e.1)).sum::<u64>() + values.len() as u64 * bits_for(tags.len());
    (dict, per_tag)
}

fn main() -> std::io::Result<()> {
    let dir = std::env::args().nth(1).expect("usage: block_widths <base-dir>");
    let snap = DiskSnapshot::open(&dir)?;
    for perm in [Perm::Spo, Perm::Pso, Perm::Pos, Perm::Osp] {
        let (mut blocks, mut cur, mut alt, mut mixed) = (0u64, [0u64; 3], [0u64; 3], [0u64; 3]);
        // Per column: block-local dictionary, per-tag frame of reference, and the best of all four per block.
        let (mut dict, mut per_tag, mut best) = ([0u64; 3], [0u64; 3], [0u64; 3]);
        let mut distinct_sum = [0u64; 3];
        let mut rows: Vec<[TermId; 3]> = Vec::with_capacity(256);
        let mut flush = |rows: &mut Vec<[TermId; 3]>| {
            if rows.is_empty() { return }
            blocks += 1;
            // Column 0: deltas from the previous row (the block's first row is the base).
            let mut prev = rows[0][0].0;
            let mut dmax = 0;
            for r in rows.iter() { dmax = dmax.max(r[0].0 - prev); prev = r[0].0 }
            cur[0] += width(dmax) * rows.len() as u64;
            alt[0] += width(dmax) * rows.len() as u64;
            for c in 1..3 {
                let (lo, hi) = rows.iter().fold((u64::MAX, 0), |(l, h), r| (l.min(r[c].0), h.max(r[c].0)));
                cur[c] += width(hi - lo) * rows.len() as u64;
                let (lo2, hi2) = rows.iter().fold((u64::MAX, 0), |(l, h), r| { let v = r[c].0.rotate_left(4); (l.min(v), h.max(v)) });
                // Per block and column the encoder would pick the narrower of the two (one flag bit per column).
                alt[c] += width(hi - lo).min(width(hi2 - lo2)) * rows.len() as u64;
                let col: Vec<u64> = rows.iter().map(|r| r[c].0).collect();
                let (d, t) = dict_and_per_tag_bits(&col);
                dict[c] += d;
                per_tag[c] += t;
                best[c] += (width(hi - lo).min(width(hi2 - lo2)) * rows.len() as u64).min(d).min(t);
                let mut u = col.clone(); u.sort_unstable(); u.dedup(); distinct_sum[c] += u.len() as u64;
                if (lo >> 60) != (hi >> 60) { mixed[c] += 1 }
            }
            rows.clear();
        };
        let mut n = 0u64;
        for batch in snap.scan(perm, &[]) {
            let b = batch?;
            for i in 0..b.len() {
                rows.push([b.cols[0][i], b.cols[1][i], b.cols[2][i]]);
                if rows.len() == 256 { flush(&mut rows) }
            }
            n += b.len() as u64;
        }
        flush(&mut rows);
        best[0] = cur[0];
        let mean = |bits: &[u64; 3]| bits.map(|b| format!("{:.1}", b as f64 / n as f64)).join(",");
        let bytes = |bits: &[u64; 3]| bits.iter().sum::<u64>() / 8;
        println!(
            "{{\"order\":\"{perm:?}\",\"rows\":{n},\"blocks\":{blocks},\"mean_width_current\":[{}],\"mean_width_rotated\":[{}],\"mean_bits_dict\":[-,{}],\"mean_bits_per_tag\":[-,{}],\"mean_distinct_per_block\":[-,{:.1},{:.1}],\"payload_bytes_current\":{},\"payload_bytes_rotated\":{},\"payload_bytes_best_of_four\":{},\"blocks_mixing_tags\":[0,{},{}]}}",
            mean(&cur), mean(&alt), mean(&dict).split(',').skip(1).collect::<Vec<_>>().join(","), mean(&per_tag).split(',').skip(1).collect::<Vec<_>>().join(","),
            distinct_sum[1] as f64 / blocks as f64, distinct_sum[2] as f64 / blocks as f64, bytes(&cur), bytes(&alt), bytes(&best), mixed[1], mixed[2]
        );
    }
    Ok(())
}
