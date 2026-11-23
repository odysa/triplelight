//! Storage read-path microbenchmark (#105, storage lane; owner Alex). Times the access paths queries use on a disk
//! base, in one process, so a profiler (samply) can attribute time to block reads, checksums, decode, validation and
//! batch assembly:
//!
//! 1. `full_<order>`: a prefix-free scan of every kept order (first pass: block-cache misses; second pass: hits while
//!    the cache holds the order, else misses again);
//! 2. `prefix1_<order>` / `prefix2_<order>`: the first batch (up to 4096 rows) of scans on `--keys` sampled one- and
//!    two-column prefixes (spread evenly over the order), the shape of index nested-loop probes;
//! 3. `seek_<order>`: `seek` with a one-column prefix and a rising target, the shape of leapfrog joins.
//!
//!   cargo run --release -p triplelight-storage --example scan_bench -- <base-dir> [--keys 20000] [--passes 2]
//!
//! Prints one JSON object per phase: seconds, rows, calls and a checksum of the rows read (so the work is not elided,
//! and two builds can be compared for identical results).
use std::time::Instant;
use triplelight_core::model::{Perm, TermId};
use triplelight_storage::{DiskSnapshot, Snapshot};

fn drain(snap: &DiskSnapshot, perm: Perm, prefix: &[TermId], sum: &mut u64) -> std::io::Result<u64> {
    drain_batches(snap, perm, prefix, usize::MAX, sum)
}

/// Reads at most `batches` batches (4096 rows each): a probe reads the start of a range, not all of a huge group.
fn drain_batches(snap: &DiskSnapshot, perm: Perm, prefix: &[TermId], batches: usize, sum: &mut u64) -> std::io::Result<u64> {
    let mut rows = 0u64;
    for batch in snap.scan(perm, prefix).take(batches) {
        let b = batch?;
        rows += b.len() as u64;
        for c in &b.cols { for v in c { *sum = sum.wrapping_mul(31).wrapping_add(v.0) } }
    }
    Ok(rows)
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dir = args.get(1).expect("usage: scan_bench <base-dir> [--keys N] [--passes N]");
    let opt = |name: &str, d: usize| args.iter().position(|a| a == name).map(|i| args[i + 1].parse().expect(name)).unwrap_or(d);
    let (keys, passes) = (opt("--keys", 20000), opt("--passes", 2));
    let t = Instant::now();
    let snap = DiskSnapshot::open(dir)?;
    println!("{{\"phase\":\"open\",\"seconds\":{:.4}}}", t.elapsed().as_secs_f64());
    let report = |phase: String, secs: f64, rows: u64, calls: u64, sum: u64| {
        println!("{{\"phase\":\"{phase}\",\"seconds\":{secs:.4},\"rows\":{rows},\"calls\":{calls},\"checksum\":{sum}}}");
    };
    // Sample prefixes from SPO's rows: every k-th row gives keys spread over the table.
    let total = snap.count(Perm::Spo, &[]).map_err(std::io::Error::other)?;
    let step = (total / keys.max(1) as u64).max(1);
    let mut sample: Vec<[TermId; 3]> = Vec::with_capacity(keys);
    let mut i = 0u64;
    for batch in snap.scan(Perm::Spo, &[]) {
        let b = batch?;
        for r in 0..b.len() {
            if i % step == 0 && sample.len() < keys { sample.push([b.cols[0][r], b.cols[1][r], b.cols[2][r]]) }
            i += 1;
        }
    }
    for perm in Perm::ALL {
        let order = perm.order();
        // A row's values in this order's key layout.
        let key = |row: &[TermId; 3]| -> [TermId; 3] { std::array::from_fn(|k| row[order[k]]) };
        for pass in 0..passes {
            let (t, mut sum) = (Instant::now(), 0u64);
            let rows = drain(&snap, perm, &[], &mut sum)?;
            report(format!("full_{perm:?}_pass{pass}"), t.elapsed().as_secs_f64(), rows, 1, sum);
        }
        for width in [1usize, 2] {
            let (t, mut sum, mut rows) = (Instant::now(), 0u64, 0u64);
            for row in &sample { rows += drain_batches(&snap, perm, &key(row)[..width], 1, &mut sum)? }
            report(format!("prefix{width}_{perm:?}"), t.elapsed().as_secs_f64(), rows, sample.len() as u64, sum);
        }
        let (t, mut sum, mut calls) = (Instant::now(), 0u64, 0u64);
        for row in &sample {
            let k = key(row);
            let mut target = TermId(0);
            // Up to 64 rising seeks inside the group, as a leapfrog join would make.
            for _ in 0..64 {
                calls += 1;
                match snap.seek(perm, &k[..1], target).map_err(std::io::Error::other)? {
                    Some(v) => { sum = sum.wrapping_mul(31).wrapping_add(v.0); if v.0 == u64::MAX { break } target = TermId(v.0 + 1) }
                    None => break,
                }
            }
        }
        report(format!("seek_{perm:?}"), t.elapsed().as_secs_f64(), 0, calls, sum);
    }
    Ok(())
}
