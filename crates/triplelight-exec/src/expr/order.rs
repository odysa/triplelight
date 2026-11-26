//! ORDER BY: the order of a solution sequence under its keys, with LIMIT as top-k.
use std::cmp::Ordering;
use triplelight_rdf::Term;

/// A key and its number when it is a numeric literal, decoded once (`num`, as `order_cmp` decodes it), so that sorting
/// many numeric keys does not parse both lexical forms on every comparison.
type Key = (Option<super::Num>, Option<Term>);

fn key_of(t: Option<Term>) -> Key { (t.as_ref().and_then(super::num), t) }

/// `order_cmp` on two keys. Two numbers take `order_num_cmp`, which is what `order_cmp` computes for two numeric
/// literals; anything else takes `order_cmp` itself. So the result is `order_cmp`'s in every case.
fn key_cmp(a: &Key, b: &Key) -> Ordering {
    if let (Some(x), Some(y)) = (a.0, b.0) { return super::order_num_cmp(x, y) }
    super::order_cmp(a.1.as_ref(), b.1.as_ref())
}

/// The indices of `n` rows in the order of their keys (`desc[k]`: key `k` descends), only the first `limit` if given.
/// `key(i, k)` is row `i`'s `k`-th key. Ties keep input order, so the result is that of a stable sort.
///
/// Top-k with more than one key: a row can be among the first `limit` only if its first key is no later than the
/// `limit`-th smallest first key, so the later keys are asked only for those rows (for an ORDER BY on a value, then on
/// an IRI, the IRIs of every other row are never decoded).
pub(crate) fn order(n: usize, desc: &[bool], limit: Option<usize>, mut key: impl FnMut(usize, usize) -> Option<Term>) -> Vec<usize> {
    let dir = |k: usize, c: Ordering| if desc[k] { c.reverse() } else { c };
    let mut idx: Vec<usize> = (0..n).collect();
    let mut keys: Vec<Vec<Key>> = vec![Vec::new(); n];
    match limit {
        Some(top) if top > 0 && top < n && desc.len() > 1 => {
            let first: Vec<Key> = (0..n).map(|i| key_of(key(i, 0))).collect();
            let first_cmp = |a: &Key, b: &Key| dir(0, key_cmp(a, b));
            let mut by_first = idx.clone();
            by_first.select_nth_unstable_by(top - 1, |a, b| first_cmp(&first[*a], &first[*b]));
            let bound = first[by_first[top - 1]].clone();
            idx.retain(|&i| first_cmp(&first[i], &bound).is_le());
            for (i, f) in first.into_iter().enumerate() { if idx.binary_search(&i).is_ok() { keys[i].push(f) } }
            for &i in &idx { for k in 1..desc.len() { let v = key_of(key(i, k)); keys[i].push(v) } }
        }
        _ => for (i, row_keys) in keys.iter_mut().enumerate() { *row_keys = (0..desc.len()).map(|k| key_of(key(i, k))).collect() },
    }
    let cmp = |a: &usize, b: &usize| -> Ordering {
        for k in 0..desc.len() {
            let c = dir(k, key_cmp(&keys[*a][k], &keys[*b][k]));
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

#[cfg(test)]
mod tests {
    use super::*;
    use triplelight_rdf::{Literal, NamedNode};

    /// The answer `order` must give, computed the plain way: a stable sort of all rows by `order_cmp` on each key in
    /// turn (stable, so ties keep input order), then the first `limit`.
    fn order_reference(n: usize, desc: &[bool], limit: Option<usize>, key: &dyn Fn(usize, usize) -> Option<Term>) -> Vec<usize> {
        let keys: Vec<Vec<Option<Term>>> = (0..n).map(|i| (0..desc.len()).map(|k| key(i, k)).collect()).collect();
        let mut idx: Vec<usize> = (0..n).collect();
        idx.sort_by(|&a, &b| {
            for k in 0..desc.len() {
                let c = super::super::order_cmp(keys[a][k].as_ref(), keys[b][k].as_ref());
                let c = if desc[k] { c.reverse() } else { c };
                if c.is_ne() { return c }
            }
            std::cmp::Ordering::Equal
        });
        if let Some(top) = limit { idx.truncate(top) }
        idx
    }

    /// A small xorshift, so the differential cases are the same on every run.
    struct Rng(u64);
    impl Rng { fn next(&mut self) -> u64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0 } }

    fn typed(v: &str, t: &str) -> Option<Term> {
        Some(Literal::new_typed_literal(v, NamedNode::new_unchecked(format!("http://www.w3.org/2001/XMLSchema#{t}"))).into())
    }

    /// Numbers of every type (mixed ints and floats around 2^24, where promotion is not transitive), signed zeros,
    /// NaN and infinities, invalid lexical forms, strings, IRIs and unbound: the cached comparator must order every mix
    /// exactly as `order_cmp` alone, with and without LIMIT, on one key and on two.
    #[test]
    fn cached_numbers_order_exactly_as_order_cmp() {
        let pool: Vec<Option<Term>> = vec![
            typed("16777216", "integer"), typed("16777217", "integer"), typed("16777216", "float"), typed("16777217.0", "double"),
            typed("1", "integer"), typed("1.0", "decimal"), typed("1.0", "float"), typed("1", "float"), typed("1E0", "double"),
            typed("-0", "float"), typed("0", "float"), typed("-0.0", "double"), typed("0", "integer"),
            typed("NaN", "float"), typed("NaN", "double"), typed("INF", "float"), typed("-INF", "double"),
            typed("0.1", "float"), typed("0.1", "double"), typed("0.30000001", "float"), typed("0.3", "float"),
            typed("abc", "integer"), typed("1.2.3", "decimal"), typed(" 2", "integer"), typed("2 ", "float"),
            Some(Literal::new_simple_literal("7").into()), Some(NamedNode::new_unchecked("urn:x").into()), None,
        ];
        let mut rng = Rng(0x5eed_0bad_cafe_0001);
        let mut panicked = 0;
        for trial in 0..400 {
            let n = 1 + (rng.next() % 60) as usize;
            let narrow = trial % 3 == 0; // every value from a few, so ties reach the second key and input order
            let pick = |rng: &mut Rng| pool[(rng.next() % if narrow { 6 } else { pool.len() as u64 }) as usize].clone();
            let rows: Vec<[Option<Term>; 2]> = (0..n).map(|_| [pick(&mut rng), pick(&mut rng)]).collect();
            for keys in [1usize, 2] {
                let desc: Vec<bool> = (0..keys).map(|k| (trial + k) % 2 == 0).collect();
                for limit in [None, Some(1), Some(3), Some(n / 2), Some(n + 1)] {
                    // A comparator that is not a total order can make the standard sorts panic; both versions run under
                    // `catch_unwind` so such a regression in `order_cmp` reports here instead of aborting the test.
                    let run = |f: fn(usize, &[bool], Option<usize>, &dyn Fn(usize, usize) -> Option<Term>) -> Vec<usize>| {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(n, &desc, limit, &|i, k| rows[i][k].clone())))
                    };
                    let new = run(|n, d, l, key| order(n, d, l, key));
                    let old = run(order_reference);
                    match (new, old) {
                        (Ok(a), Ok(b)) => assert_eq!(a, b, "trial {trial}, keys {keys}, limit {limit:?}"),
                        (Err(_), Err(_)) => panicked += 1, // `order_cmp` is a total order since #347: none expected
                        (a, b) => panic!("trial {trial}, keys {keys}, limit {limit:?}: new {:?}, old {:?}", a.is_ok(), b.is_ok()),
                    }
                }
            }
        }
        assert_eq!(panicked, 0, "{panicked} sorts panicked: order_cmp should be a total order");
    }

    /// The comparator itself, on every pair of the pool: exactly `order_cmp`.
    #[test]
    fn key_cmp_is_order_cmp_on_every_pair() {
        let pool = [typed("16777216", "integer"), typed("16777217", "integer"), typed("16777216", "float"), typed("-0", "float"),
            typed("0", "double"), typed("NaN", "float"), typed("INF", "double"), typed("1.0", "decimal"), typed("1", "integer"),
            typed("x", "float"), Some(Literal::new_simple_literal("1").into()), Some(NamedNode::new_unchecked("urn:a").into()), None];
        for a in &pool { for b in &pool {
            assert_eq!(key_cmp(&key_of(a.clone()), &key_of(b.clone())), super::super::order_cmp(a.as_ref(), b.as_ref()), "{a:?} {b:?}");
        } }
    }
}
