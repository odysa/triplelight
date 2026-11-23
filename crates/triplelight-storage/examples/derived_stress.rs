//! Derived-order stress test (plan amendment 1, A8 / gap W9; owner Alex). Runs the expensive paths of SOP and OPS on a
//! disk store, so a four-order store (SOP and OPS derived, `derived_order.rs`) can be compared with a six-order store of
//! the same data (both stored), call for call:
//!
//! 1. one-column prefixes on the `--top` subjects and objects with the most triples: a full `scan(SOP, [s])` and
//!    `scan(OPS, [o])` each (the regroup path on a four-order store);
//! 2. on the same keys, a leapfrog-style seek walk: `seek(SOP, [s], t)` over every distinct object `t` of `s` in rising
//!    order (and `seek(OPS, [o], t)` over predicates), which the seek memo turns from quadratic into linear;
//! 3. prefix-free scans of SOP and OPS (whole table, regrouped one first-column group at a time).
//!
//!   TRIPLELIGHT_TRACE_DERIVED=1 /usr/bin/time -v cargo run --release -p triplelight-storage --example derived_stress \
//!     -- <store-dir> [--top 50] [--skip-full]
//!
//! Prints one JSON object: per phase, wall seconds, rows and calls, and the derived-order trace counters (regroup scans,
//! rows, groups, the largest group, memo hits and misses, sort time). Peak RSS comes from the time utility around it.
//! Rows are checked: every SOP/OPS row must have the requested prefix, and rows must come out in key order.
use std::time::Instant;
use triplelight_core::model::{Perm, TermId};
use triplelight_storage::{DiskSnapshot, Snapshot};

fn top_keys(snap: &DiskSnapshot, perm: Perm, n: usize) -> std::io::Result<Vec<(TermId, u64)>> {
    // One pass over the order whose first column is the key, counting each key's run.
    let mut best: Vec<(u64, TermId)> = Vec::new();
    let (mut cur, mut len) = (None::<TermId>, 0u64);
    let mut push = |k: Option<TermId>, len: u64, best: &mut Vec<(u64, TermId)>| {
        if let Some(k) = k {
            best.push((len, k));
            if best.len() > 4 * n { best.sort_unstable_by(|a, b| b.cmp(a)); best.truncate(n); }
        }
    };
    for batch in snap.scan(perm, &[]) {
        let batch = batch?;
        for &k in &batch.cols[0] {
            if Some(k) == cur { len += 1 } else { push(cur, len, &mut best); cur = Some(k); len = 1 }
        }
    }
    push(cur, len, &mut best);
    best.sort_unstable_by(|a, b| b.cmp(a));
    best.truncate(n);
    Ok(best.into_iter().map(|(len, k)| (k, len)).collect())
}

/// Consumes a scan, checking prefix and order. Returns the number of rows.
fn drain(snap: &DiskSnapshot, perm: Perm, prefix: &[TermId]) -> std::io::Result<u64> {
    let (mut rows, mut last) = (0u64, None::<[TermId; 3]>);
    for batch in snap.scan(perm, prefix) {
        let b = batch?;
        for i in 0..b.len() {
            let row = [b.cols[0][i], b.cols[1][i], b.cols[2][i]];
            if row[..prefix.len()] != *prefix { return Err(std::io::Error::other(format!("{perm:?}: row outside prefix"))) }
            if last.is_some_and(|l| l >= row) { return Err(std::io::Error::other(format!("{perm:?}: rows out of order"))) }
            last = Some(row);
            rows += 1;
        }
    }
    Ok(rows)
}

/// A leapfrog-style walk: seek every distinct second-column value of `key` in rising order. Returns the seek count.
fn seek_walk(snap: &DiskSnapshot, perm: Perm, key: TermId) -> std::io::Result<u64> {
    let (mut seeks, mut target) = (0u64, TermId(0));
    while let Some(v) = snap.seek(perm, &[key], target)? {
        seeks += 1;
        if v.0 == u64::MAX { break }
        target = TermId(v.0 + 1);
    }
    Ok(seeks)
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dir = args.get(1).expect("usage: derived_stress <store-dir> [--top N] [--skip-full] [--cache-mb N]");
    let top = args.iter().position(|a| a == "--top").map(|i| args[i + 1].parse().expect("--top N")).unwrap_or(50usize);
    let skip_full = args.iter().any(|a| a == "--skip-full");
    if let Some(i) = args.iter().position(|a| a == "--cache-mb") {
        triplelight_storage::disk_index::set_cache_budget_mb(args[i + 1].parse().expect("--cache-mb N"))?;
    }
    let t = Instant::now();
    let snap = DiskSnapshot::open(dir)?;
    let open_s = t.elapsed().as_secs_f64();
    let trace_on = triplelight_storage::derived_trace::enabled();
    let _ = triplelight_storage::derived_trace::take();
    let mut phases = Vec::new();
    let mut phase = |name: &str, f: &mut dyn FnMut() -> std::io::Result<(u64, u64)>| -> std::io::Result<()> {
        let t = Instant::now();
        let (rows, calls) = f()?;
        let secs = t.elapsed().as_secs_f64();
        let trace = triplelight_storage::derived_trace::take();
        eprintln!("{name}: {secs:.3} s, {rows} rows, {calls} calls, {trace}");
        phases.push(format!("{{\"phase\":\"{name}\",\"seconds\":{secs:.4},\"rows\":{rows},\"calls\":{calls},\"trace\":\"{trace}\"}}"));
        Ok(())
    };
    let subjects = top_keys(&snap, Perm::Spo, top)?;
    let objects = top_keys(&snap, Perm::Osp, top)?;
    phase("sop_prefix_scan", &mut || { let mut r = 0; for (s, _) in &subjects { r += drain(&snap, Perm::Sop, &[*s])? } Ok((r, subjects.len() as u64)) })?;
    phase("ops_prefix_scan", &mut || { let mut r = 0; for (o, _) in &objects { r += drain(&snap, Perm::Ops, &[*o])? } Ok((r, objects.len() as u64)) })?;
    phase("sop_seek_walk", &mut || { let mut n = 0; for (s, _) in &subjects { n += seek_walk(&snap, Perm::Sop, *s)? } Ok((0, n)) })?;
    phase("ops_seek_walk", &mut || { let mut n = 0; for (o, _) in &objects { n += seek_walk(&snap, Perm::Ops, *o)? } Ok((0, n)) })?;
    if !skip_full {
        phase("sop_full_scan", &mut || Ok((drain(&snap, Perm::Sop, &[])?, 1)))?;
        phase("ops_full_scan", &mut || Ok((drain(&snap, Perm::Ops, &[])?, 1)))?;
    }
    let keys = |v: &[(TermId, u64)]| v.iter().map(|(_, n)| n.to_string()).collect::<Vec<_>>().join(",");
    println!(
        "{{\"store\":\"{}\",\"open_s\":{open_s:.3},\"trace_enabled\":{trace_on},\"top\":{top},\"subject_group_sizes\":[{}],\"object_group_sizes\":[{}],\"phases\":[{}]}}",
        dir.replace('"', "'"), keys(&subjects), keys(&objects), phases.join(",")
    );
    Ok(())
}
