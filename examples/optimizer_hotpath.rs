//! Explicit phase profiling, never a throughput benchmark. Run with RDFDB_PROFILE_PLAN=1.
use triplelight::exec::{Options, QueryResult, query_with};
use triplelight::opt::Stats;
use triplelight::storage::MemorySnapshot;
use std::{fs::File, io::BufReader, time::Instant};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 3 {
        return Err("usage: optimizer_hotpath DATA.nt QUERY.rq...".into());
    }
    let snapshot = MemorySnapshot::from_ntriples(BufReader::new(File::open(&args[1])?))?;
    let stats = Stats::from_snapshot(&snapshot)?;
    eprintln!(
        "synopsis triples={} characteristic_sets={}",
        stats.triple_count,
        stats.characteristic_sets.len()
    );
    for file in &args[2..] {
        let text = std::fs::read_to_string(file)?;
        for mode in [false, true] {
            for sample in 0..5 {
                eprintln!("profile_begin query={file} stats={mode} sample={sample}");
                let start = Instant::now();
                let result = query_with(
                    &snapshot,
                    &text,
                    &Options {
                        stats: mode.then_some(&stats),
                        wcoj: true,
                        raw_ids: false,
                    },
                )?;
                let query_ns = start.elapsed().as_nanos();
                let QueryResult::Solutions { rows, .. } = result else {
                    return Err("SELECT expected".into());
                };
                eprintln!("profile_end query_ns={query_ns} rows={}", rows.len());
                std::hint::black_box(rows);
            }
        }
    }
    Ok(())
}
