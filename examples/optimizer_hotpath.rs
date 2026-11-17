//! Explicit phase profiling, never an HTTP throughput benchmark.
//! Set RDFDB_PROFILE_PLAN=1; DATA may be an N-Triples file or disk snapshot directory.
use rdfdb::exec::{Options, QueryResult, query_with};
use rdfdb::opt::Stats;
use rdfdb::storage::{DiskSnapshot, MemorySnapshot, Snapshot};
use sha2::{Digest, Sha256};
use std::{fs::File, io::BufReader, path::Path, time::Instant};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 3 {
        return Err("usage: optimizer_hotpath DATA_OR_DISK_DIR QUERY.rq...".into());
    }
    let load = Instant::now();
    let snapshot: Box<dyn Snapshot> = if Path::new(&args[1]).is_dir() {
        Box::new(DiskSnapshot::open(&args[1])?)
    } else {
        Box::new(MemorySnapshot::from_ntriples(BufReader::new(File::open(
            &args[1],
        )?))?)
    };
    eprintln!("load_ns={}", load.elapsed().as_nanos());
    let build = Instant::now();
    let stats = Stats::from_snapshot(&*snapshot)?;
    eprintln!(
        "synopsis triples={} characteristic_sets={} stats_build_ns={}",
        stats.triple_count,
        stats.characteristic_sets.len(),
        build.elapsed().as_nanos()
    );
    for file in &args[2..] {
        let text = std::fs::read_to_string(file)?;
        let mut reference = None;
        for sample in 0..5 {
            // Alternate order to reduce a fixed cache/order advantage.
            let modes = if sample % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            };
            for mode in modes {
                eprintln!("profile_begin query={file} stats={mode} sample={sample}");
                let start = Instant::now();
                let result = query_with(
                    &*snapshot,
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
                // Includes complete query modifiers; never compare BSBM final rows
                // to an unfiltered BGP estimate. Canonicalization is outside timing.
                let mut normalized: Vec<Vec<Option<String>>> = rows
                    .into_iter()
                    .map(|row| row.into_iter().map(|t| t.map(|t| t.to_string())).collect())
                    .collect();
                let sequence_hash =
                    format!("{:x}", Sha256::digest(format!("{normalized:?}").as_bytes()));
                normalized.sort();
                if let Some(old) = &reference {
                    if old != &normalized {
                        return Err(format!(
                            "stats/no-stats multiset mismatch: {file} sample {sample}"
                        )
                        .into());
                    }
                } else {
                    reference = Some(normalized.clone());
                }
                let multiset_hash =
                    format!("{:x}", Sha256::digest(format!("{normalized:?}").as_bytes()));
                eprintln!(
                    "profile_end query_ns={query_ns} rows={} multiset_sha256={multiset_hash} sequence_sha256={sequence_hash}",
                    normalized.len()
                );
            }
        }
    }
    Ok(())
}
