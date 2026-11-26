//! Opt-in wide-table choice diagnostic. Index and automatic arms share one built view.
//! Usage: cargo run --release --example wide_choice -- STORE QUERY.rq...
use std::time::Instant;
use triplelight::exec::{Options, QueryResult, plan_trace, query_with};
use triplelight::opt::stats::Stats;
use triplelight::storage::DiskSnapshot;
use triplelight::storage::wide_tables::{BuildOptions, WithTables};

fn answer(result: QueryResult) -> Vec<String> {
    match result {
        QueryResult::Solutions { rows, .. } => {
            let mut rows: Vec<_> = rows.into_iter().map(|row| format!("{row:?}")).collect();
            rows.sort_unstable();
            rows
        }
        other => vec![format!("{other:?}")],
    }
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(|a, b| a.total_cmp(b));
    values[values.len() / 2]
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert!(args.len() >= 3, "usage: wide_choice STORE QUERY.rq...");
    let snap = WithTables::build(DiskSnapshot::open(&args[1]).expect("store"), BuildOptions::default()).expect("tables");
    let stats = Stats::from_snapshot(&snap).expect("stats");
    println!("tables={} table_bytes={}", snap.table_count(), snap.table_bytes());
    println!("query\tplan\tindex_ms\tauto_ms\tratio\trows\tsame_bag");
    for file in &args[2..] {
        let query = std::fs::read_to_string(file).expect("query");
        let index = Options { stats: Some(&stats), ..Options::default() };
        let auto = Options { stats: Some(&stats), auto_wide_tables: true, ..Options::default() };
        let (first, trace) = plan_trace::capture(|| query_with(&snap, &query, &auto).expect("auto query"));
        let plan = if trace.iter().any(|line| line.contains("strategy=wide-auto")) { "table" } else { "index" };
        let expected = answer(query_with(&snap, &query, &index).expect("index query"));
        assert_eq!(answer(first), expected, "{file}: auto answer");
        for options in [&index, &auto] {
            query_with(&snap, &query, options).expect("warmup");
        }
        let (mut indexed, mut chosen) = (Vec::new(), Vec::new());
        for round in 0..2 {
            let order = if round == 0 { [false, true, true, false] } else { [true, false, false, true] };
            for use_auto in order {
                let options = if use_auto { &auto } else { &index };
                for _ in 0..5 {
                    let start = Instant::now();
                    let result = query_with(&snap, &query, options).expect("timed query");
                    let elapsed = start.elapsed().as_secs_f64() * 1e3;
                    assert_eq!(answer(result), expected, "{file}: changed answer");
                    if use_auto { chosen.push(elapsed) } else { indexed.push(elapsed) }
                }
            }
        }
        let (i, a) = (median(&mut indexed), median(&mut chosen));
        println!("{file}\t{plan}\t{i:.2}\t{a:.2}\t{:.3}\t{}\ttrue", a / i, expected.len());
    }
}
