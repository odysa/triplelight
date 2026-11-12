use rdfdb::exec::{query_with, Options, results::serialize, QueryResult};
use rdfdb::opt::stats::Stats;
use rdfdb::storage::MemorySnapshot;
use std::time::Instant;
fn n(r: &QueryResult) -> usize { match r { QueryResult::Solutions { rows, .. } => rows.len(), _ => 0 } }
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let s = MemorySnapshot::from_ntriples(std::io::BufReader::new(std::fs::File::open(&a[1]).unwrap())).unwrap();
    let t = Instant::now(); let st = Stats::from_snapshot(&s).unwrap(); eprintln!("stats {:?}", t.elapsed());
    for q in &a[2..] {
        let text = std::fs::read_to_string(q).unwrap();
        let mut line = format!("{q}:");
        for (name, opts) in [("bind", Options { stats: None, wcoj: false, raw_ids: false }), ("bind+wcoj", Options::default()), ("planner", Options { stats: Some(&st), wcoj: false, raw_ids: false })] {
            let mut best = f64::MAX; let mut rows = 0;
            for _ in 0..5 {
                let t = Instant::now(); let r = query_with(&s, &text, &opts).unwrap(); let _ = serialize(&r);
                best = best.min(t.elapsed().as_secs_f64() * 1e3); rows = n(&r);
            }
            line += &format!("  {name} {best:.2}ms n={rows}");
        }
        eprintln!("{line}");
    }
}
