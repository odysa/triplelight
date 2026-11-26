//! #140 A/B: wide tables forced on (`TRIPLELIGHT_FORCE_WIDE`) against the index path (`TRIPLELIGHT_DISABLE_WIDE`) on one
//! disk store. Each leg runs in a fresh child process with its switch set at launch (the switch is read from the
//! environment, which must not change while query threads run), in ABBA order. Both arms build the same tables, so
//! the process state is the same; table build time and bytes are reported separately from query timing. Every timed
//! run's full answer is compared as a bag across arms.
//!
//!   cargo run --release --example wide_ab -- STORE_DIR ROUNDS QUERY.rq...      (parent: prints the summary)
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::process::Command;
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

fn leg(store: &str, queries: &[String]) {
    let t = Instant::now();
    let disk = DiskSnapshot::open(store).expect("open store");
    let open_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let snap = WithTables::build(disk, BuildOptions::default()).expect("build tables");
    let build_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let stats = Stats::from_snapshot(&snap).expect("stats");
    let stats_ms = t.elapsed().as_secs_f64() * 1e3;
    println!("meta\topen_ms={open_ms:.0}\tbuild_ms={build_ms:.0}\tstats_ms={stats_ms:.0}\ttables={}\ttable_bytes={}\ttable_triples={}\tview_triples={}",
        snap.tables.len(), snap.tables.bytes(), snap.tables.table_triples(), snap.tables.view_triples());
    let opts = Options { stats: Some(&stats), ..Options::default() };
    for q in queries {
        let text = std::fs::read_to_string(q).expect("query file");
        for _ in 0..WARMUP { query_with(&snap, &text, &opts).expect("query"); }
        for _ in 0..RUNS {
            let t = Instant::now();
            let r = query_with(&snap, &text, &opts).expect("query");
            let ms = t.elapsed().as_secs_f64() * 1e3;
            let (rows, digest) = bag(&r);
            println!("run\t{q}\t{ms:.3}\t{rows}\t{digest:016x}");
        }
    }
}

fn median(v: &mut [f64]) -> f64 { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[v.len() / 2] }

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.get(1).map(String::as_str) == Some("--leg") { return leg(&a[2], &a[3..]) }
    let (store, rounds, queries) = (&a[1], a[2].parse::<usize>().expect("ROUNDS"), &a[3..]);
    let exe = std::env::current_exe().unwrap();
    // ABBA per round: wide, index, index, wide (then the mirror next round)
    let mut times: BTreeMap<(String, &str), Vec<f64>> = BTreeMap::new();
    let mut digests: BTreeMap<String, BTreeMap<&str, (usize, String)>> = BTreeMap::new();
    let mut builds: Vec<String> = Vec::new();
    for round in 0..rounds {
        let order: [&str; 4] = if round % 2 == 0 { ["wide", "index", "index", "wide"] } else { ["index", "wide", "wide", "index"] };
        for arm in order {
            let mut cmd = Command::new(&exe);
            cmd.arg("--leg").arg(store).args(queries).env_remove("TRIPLELIGHT_FORCE_WIDE").env_remove("TRIPLELIGHT_DISABLE_WIDE");
            if arm == "wide" { cmd.env("TRIPLELIGHT_FORCE_WIDE", "1"); } else { cmd.env("TRIPLELIGHT_DISABLE_WIDE", "1"); }
            let out = cmd.output().expect("leg");
            assert!(out.status.success(), "leg {arm} failed: {}", String::from_utf8_lossy(&out.stderr));
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                let f: Vec<&str> = line.split('\t').collect();
                match f[0] {
                    "meta" => builds.push(format!("{arm}: {}", f[1..].join(" "))),
                    "run" => {
                        times.entry((f[1].to_string(), arm)).or_default().push(f[2].parse().unwrap());
                        let d = digests.entry(f[1].to_string()).or_default();
                        let seen = (f[3].parse().unwrap(), f[4].to_string());
                        if let Some(prev) = d.get(arm) { assert_eq!(prev, &seen, "{} {arm}: answer changed between runs", f[1]) }
                        d.insert(arm, seen);
                    }
                    _ => {}
                }
            }
        }
    }
    for b in &builds { println!("build {b}") }
    println!("query\tindex_ms\twide_ms\tratio\trows\tsame_bag");
    let (mut si, mut sw) = (0.0, 0.0);
    for (q, d) in &digests {
        let mut i = times[&(q.clone(), "index")].clone();
        let mut w = times[&(q.clone(), "wide")].clone();
        let (mi, mw) = (median(&mut i), median(&mut w));
        si += mi; sw += mw;
        let same = d.get("index") == d.get("wide");
        println!("{q}\t{mi:.2}\t{mw:.2}\t{:.3}\t{}\t{same}", mw / mi, d["index"].0);
        assert!(same, "{q}: the wide path's answer differs from the index path's as a bag");
    }
    println!("sum\t{si:.2}\t{sw:.2}\t{:.3}", sw / si);
}
