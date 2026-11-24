//! ORDER BY: the order of a solution sequence under its keys, with LIMIT as top-k.
use std::cmp::Ordering;
use triplelight_rdf::Term;

/// The indices of `n` rows in the order of their keys (`desc[k]`: key `k` descends), only the first `limit` if given.
/// `key(i, k)` is row `i`'s `k`-th key. Ties keep input order, so the result is that of a stable sort.
///
/// Top-k with more than one key: a row can be among the first `limit` only if its first key is no later than the
/// `limit`-th smallest first key, so the later keys are asked only for those rows (for an ORDER BY on a value, then on
/// an IRI, the IRIs of every other row are never decoded).
pub(crate) fn order(n: usize, desc: &[bool], limit: Option<usize>, mut key: impl FnMut(usize, usize) -> Option<Term>) -> Vec<usize> {
    let dir = |k: usize, c: Ordering| if desc[k] { c.reverse() } else { c };
    let mut idx: Vec<usize> = (0..n).collect();
    let mut keys: Vec<Vec<Option<Term>>> = vec![Vec::new(); n];
    match limit {
        Some(top) if top > 0 && top < n && desc.len() > 1 => {
            let first: Vec<Option<Term>> = (0..n).map(|i| key(i, 0)).collect();
            let first_cmp = |a: &Option<Term>, b: &Option<Term>| dir(0, super::order_cmp(a.as_ref(), b.as_ref()));
            let mut by_first = idx.clone();
            by_first.select_nth_unstable_by(top - 1, |a, b| first_cmp(&first[*a], &first[*b]));
            let bound = first[by_first[top - 1]].clone();
            idx.retain(|&i| first_cmp(&first[i], &bound).is_le());
            for (i, f) in first.into_iter().enumerate() { if idx.binary_search(&i).is_ok() { keys[i].push(f) } }
            for &i in &idx { for k in 1..desc.len() { let v = key(i, k); keys[i].push(v) } }
        }
        _ => for (i, row_keys) in keys.iter_mut().enumerate() { *row_keys = (0..desc.len()).map(|k| key(i, k)).collect() },
    }
    let cmp = |a: &usize, b: &usize| -> Ordering {
        for k in 0..desc.len() {
            let c = dir(k, super::order_cmp(keys[*a][k].as_ref(), keys[*b][k].as_ref()));
            if c.is_ne() { return c }
        }
        a.cmp(b) // ties keep input order (as a stable sort would)
    };
    match limit {
        Some(top) if top < idx.len() => {
            // Stable top-k: the comparator is a total order (index tie-break), so the `top` smallest equal the
            // first `top` of a full stable sort.
            if top > 0 { idx.select_nth_unstable_by(top - 1, cmp); }
            idx.truncate(top);
            idx.sort_unstable_by(cmp);
        }
        _ => idx.sort_unstable_by(cmp),
    }
    idx
}
