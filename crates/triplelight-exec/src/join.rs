//! Joins of materialized solution sets (subquery results joined back into the outer rows, OPTIONAL's merge).
use crate::{Row, UNBOUND};
use crate::model::TermId;
use std::collections::HashMap;

/// Compatible-mapping join of two row sets (used to merge subquery results back into the outer rows). Columns bound in
/// every row on both sides are a sound hash key: two compatible mappings must agree there. The remaining columns are
/// merged with the usual compatibility check. Without such a column (partial or unbound mappings), the nested-loop
/// join is used.
pub(crate) fn join_rows(left: Vec<Row>, right: Vec<Row>) -> Vec<Row> {
    if left.is_empty() || right.is_empty() { return Vec::new() }
    let width = left[0].len();
    let key: Vec<usize> = (0..width).filter(|&i| left.iter().all(|r| r[i] != UNBOUND) && right.iter().all(|r| r[i] != UNBOUND)).collect();
    if !key.is_empty() && left.len().saturating_mul(right.len()) > 64 {
        if let [column] = key.as_slice() {
            return join_one_bound_column(&left, &right, *column);
        }
        let mut table: HashMap<Vec<TermId>, Vec<usize>> = HashMap::new();
        for (j, b) in right.iter().enumerate() { table.entry(key.iter().map(|&i| b[i]).collect()).or_default().push(j) }
        let mut out = Vec::new();
        let mut k: Vec<TermId> = Vec::with_capacity(key.len());
        for a in &left {
            k.clear(); k.extend(key.iter().map(|&i| a[i]));
            let Some(matches) = table.get(k.as_slice()) else { continue };
            out.extend(matches.iter().filter_map(|&j| merge_compatible(a, &right[j])));
        }
        return out;
    }
    let mut out = Vec::new();
    for a in &left {
        out.extend(right.iter().filter_map(|b| merge_compatible(a, b)));
    }
    out
}

fn join_one_bound_column(left: &[Row], right: &[Row], column: usize) -> Vec<Row> {
    const END: usize = usize::MAX;
    let mut first = HashMap::new();
    let mut next = vec![END; right.len()];
    // Build in reverse so each key's chain follows the original right-row order.
    for (i, row) in right.iter().enumerate().rev() {
        next[i] = first.insert(row[column], i).unwrap_or(END);
    }
    let mut out = Vec::new();
    for row in left {
        let mut i = first.get(&row[column]).copied().unwrap_or(END);
        while i != END {
            if let Some(merged) = merge_compatible(row, &right[i]) {
                out.push(merged);
            }
            i = next[i];
        }
    }
    out
}

/// Merge two mappings: `a` extended with every binding of `b`, or `None` when they disagree on a variable both bind.
#[inline(always)]
pub(crate) fn merge_compatible(a: &Row, b: &Row) -> Option<Row> {
    let mut r = a.clone();
    for (i, &y) in b.iter().enumerate() {
        if y == UNBOUND { continue }
        if r[i] == UNBOUND { r[i] = y } else if r[i] != y { return None }
    }
    Some(r)
}

#[cfg(test)]
mod join_rows_bench {
    use super::*;
    use std::time::Instant;

    /// The previous `join_rows`, kept only as the microbenchmark baseline.
    fn join_rows_old(left: Vec<Row>, right: Vec<Row>) -> Vec<Row> {
        if left.is_empty() || right.is_empty() { return Vec::new() }
        let width = left[0].len();
        let key: Vec<usize> = (0..width).filter(|&i| left.iter().all(|r| r[i] != UNBOUND) && right.iter().all(|r| r[i] != UNBOUND)).collect();
        if !key.is_empty() && left.len().saturating_mul(right.len()) > 64 {
            let mut table: HashMap<Vec<TermId>, Vec<usize>> = HashMap::new();
            for (j, b) in right.iter().enumerate() { table.entry(key.iter().map(|&i| b[i]).collect()).or_default().push(j) }
            let mut out = Vec::new();
            let mut k: Vec<TermId> = Vec::with_capacity(key.len());
            for a in &left {
                k.clear(); k.extend(key.iter().map(|&i| a[i]));
                let Some(matches) = table.get(k.as_slice()) else { continue };
                'm: for &j in matches {
                    let b = &right[j];
                    let mut r = a.clone();
                    for (i, &y) in b.iter().enumerate() {
                        if y == UNBOUND { continue }
                        if r[i] == UNBOUND { r[i] = y } else if r[i] != y { continue 'm }
                    }
                    out.push(r);
                }
            }
            return out;
        }
        let mut out = Vec::new();
        for a in &left {
            'r: for b in &right {
                let mut r = a.clone();
                for (i, &y) in b.iter().enumerate() {
                    if y == UNBOUND { continue }
                    if r[i] == UNBOUND { r[i] = y } else if r[i] != y { continue 'r }
                }
                out.push(r);
            }
        }
        out
    }
    fn rows(n: u64, offset: u64) -> Vec<Row> {
        // Column 0 is a join key bound everywhere; column 1 is bound on one side; column 2 conflicts on about 1 in 5.
        (0..n).map(|i| vec![TermId(1 + i % 400), if offset == 0 { TermId(7 + i % 5) } else { UNBOUND }, TermId(100 + (i + offset) % 5)]).collect()
    }

    #[test]
    fn single_bound_column_join_matches_legacy_order() {
        let left = [
            [TermId(1), TermId(4), UNBOUND],
            [TermId(2), UNBOUND, TermId(9)],
            [TermId(3), UNBOUND, UNBOUND],
        ].repeat(4).into_iter().map(Vec::from).collect::<Vec<_>>();
        let right = [
            [TermId(1), UNBOUND, TermId(7)],
            [TermId(1), TermId(5), TermId(8)],
            [TermId(2), TermId(6), TermId(9)],
            [TermId(1), TermId(4), TermId(8)],
            [TermId(2), UNBOUND, TermId(9)],
            [TermId(2), UNBOUND, TermId(9)],
        ].repeat(2).into_iter().map(Vec::from).collect::<Vec<_>>();
        let expected = join_rows_old(left.clone(), right.clone());
        assert_eq!(expected.len(), 40);
        assert_eq!(join_rows(left, right), expected);
    }

    /// `cargo test --release -p triplelight-exec join_rows_bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn merge_helper_is_not_slower_than_the_inlined_join() {
        for (name, l, r) in [("hash path", (rows(4000, 0), rows(4000, 1)), 0), ("nested loop", (rows(60, 0), rows(60, 1)), 0)].map(|(n, (a, b), _)| (n, a, b)) {
            assert_eq!(join_rows_old(l.clone(), r.clone()), join_rows(l.clone(), r.clone()), "{name}: identical output");
            let out_rows = join_rows(l.clone(), r.clone()).len().max(1) as f64;
            let (mut old, mut new) = (Vec::new(), Vec::new());
            for round in 0..9 {
                for variant in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                    let (a, b) = (l.clone(), r.clone());
                    let t = Instant::now();
                    let out = if variant == 0 { join_rows_old(a, b) } else { join_rows(a, b) };
                    let ns = t.elapsed().as_nanos() as f64 / out_rows;
                    std::hint::black_box(&out);
                    if variant == 0 { old.push(ns) } else { new.push(ns) }
                }
            }
            old.sort_by(f64::total_cmp);
            new.sort_by(f64::total_cmp);
            let (o, n) = (old[4], new[4]);
            eprintln!("join_rows {name}, ns per output row (median of 9, interleaved): old {o:.2}, new {n:.2}, ratio {:.3}", n / o);
            assert!(n <= o * 1.05, "{name}: new is more than 5% slower");
        }
    }
}
