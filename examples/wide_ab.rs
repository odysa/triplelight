//! #140 A/B: wide tables forced on (`Options::force_wide_tables`) against the index path, on one disk store in one
//! process: the store is opened, the tables built and the statistics computed once, then every query alternates
//! arms in ABBA order (wide, index, index, wide, then mirrored) for `ROUNDS` rounds of `RUNS` timed runs each, after
//! `WARMUP` runs per arm. Table build time and bytes are reported apart from query timing. Every timed run's full
//! answer is compared as a bag across arms; a difference stops the harness.
//!
//!   cargo run --release --example wide_ab -- STORE_DIR ROUNDS QUERY.rq...
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::time::Instant;
use triplelight::exec::{query_with, Options, QueryResult};
use triplelight::opt::stats::Stats;
use triplelight::storage::wide_tables::{BuildOptions, WithTables};
use triplelight::storage::DiskSnapshot;

const WARMUP: usize = 2;
const RUNS: usize = 5;

/// Order-insensitive digest of an answer: the hash of its sorted rows (a bag).
fn bag(r: &QueryResult) -> (usize, u64) {
    let QueryResult::Solutions { rows, .. } = r else { return (0, 0) };
    let mut v: Vec<String> = rows.iter().map(|row| format!("{row:?}")).collect();
    v.sort_unstable();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    (v.len(), h.finish())
}

fn median(v: &mut [f64]) -> f64 { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[v.len() / 2] }

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (store, rounds, queries) = (&a[1], a[2].parse::<usize>().expect("ROUNDS"), &a[3..]);
    let t = Instant::now();
    let disk = DiskSnapshot::open(store).expect("open store");
    let open_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let snap = WithTables::build(disk, BuildOptions::default()).expect("build tables");
    let build_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let stats = Stats::from_snapshot(&snap).expect("stats");
    let stats_ms = t.elapsed().as_secs_f64() * 1e3;
    println!("open_ms={open_ms:.0} build_ms={build_ms:.0} stats_ms={stats_ms:.0} tables={} table_bytes={} table_triples={} view_triples={}",
        snap.table_count(), snap.table_bytes(), snap.table_triples(), snap.view_triples());
    let arm_opts = |wide: bool| Options { stats: Some(&stats), force_wide_tables: wide, ..Options::default() };
    // ROUNDS = 0: profiling mode, the table arm only, 40 runs per query, no comparison
    if rounds == 0 {
        for q in queries {
            let text = std::fs::read_to_string(q).expect("query file");
            let t = Instant::now();
            for _ in 0..40 { query_with(&snap, &text, &arm_opts(true)).expect("query"); }
            println!("{q}\twide-only 40 runs\t{:.1} ms each", t.elapsed().as_secs_f64() * 1e3 / 40.0);
        }
        return;
    }
    println!("query\tindex_ms\twide_ms\tratio\trows\tsame_bag");
    let (mut si, mut sw) = (0.0, 0.0);
    for q in queries {
        let text = std::fs::read_to_string(q).expect("query file");
        for wide in [true, false] { for _ in 0..WARMUP { query_with(&snap, &text, &arm_opts(wide)).expect("query"); } }
        let mut times: BTreeMap<bool, Vec<f64>> = BTreeMap::new();
        let mut digest: BTreeMap<bool, (usize, u64)> = BTreeMap::new();
        for round in 0..rounds {
            let order = if round % 2 == 0 { [true, false, false, true] } else { [false, true, true, false] };
            for wide in order {
                for _ in 0..RUNS {
                    let t = Instant::now();
                    let r = query_with(&snap, &text, &arm_opts(wide)).expect("query");
                    times.entry(wide).or_default().push(t.elapsed().as_secs_f64() * 1e3);
                    let d = bag(&r);
                    if let Some(prev) = digest.insert(wide, d) { assert_eq!(prev, d, "{q}: answer changed between runs") }
                }
            }
        }
        let (mi, mw) = (median(times.get_mut(&false).unwrap()), median(times.get_mut(&true).unwrap()));
        si += mi; sw += mw;
        let same = digest[&false] == digest[&true];
        println!("{q}\t{mi:.2}\t{mw:.2}\t{:.3}\t{}\t{same}", mw / mi, digest[&false].0);
        assert!(same, "{q}: the wide path's answer differs from the index path's as a bag");
    }
    println!("sum\t{si:.2}\t{sw:.2}\t{:.3}", sw / si);
}
