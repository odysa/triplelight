//! `triplelight query <data> <query>` — run one query and print JSON results.
//! With the `server` feature: `triplelight serve` / `serve-store` speak the SPARQL 1.1 protocol (see `server.rs`).
use triplelight::exec::{query, results::serialize};
use triplelight::storage::{MemorySnapshot, DiskSnapshot, Dictionary, Snapshot, StorageError};
use triplelight::model::{Perm, TermId, TripleBatch};
use oxrdf::{Term, TermRef};

#[cfg(target_os = "linux")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub(crate) enum Backend { Memory(MemorySnapshot), Disk(DiskSnapshot) }
impl Dictionary for Backend {
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError> { match self { Self::Memory(s) => s.lookup(term), Self::Disk(s) => s.lookup(term) } }
    fn resolve(&self, id: TermId) -> Result<Term, StorageError> { match self { Self::Memory(s) => s.resolve(id), Self::Disk(s) => s.resolve(id) } }
    fn with_term(&self, id: TermId, f: &mut dyn FnMut(TermRef<'_>)) -> Result<(), StorageError> { match self { Self::Memory(s) => s.with_term(id, f), Self::Disk(s) => s.with_term(id, f) } }
    fn with_terms(&self, ids: &[TermId], f: &mut dyn FnMut(usize, TermRef<'_>)) -> Result<(), StorageError> { match self { Self::Memory(s) => s.with_terms(ids, f), Self::Disk(s) => s.with_terms(ids, f) } }
}
impl Snapshot for Backend {
    fn scan<'a>(&'a self, p: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item=Result<TripleBatch, StorageError>> + 'a> {
        match self { Self::Memory(s) => s.scan(p,prefix), Self::Disk(s) => s.scan(p,prefix) }
    }
    fn scan_for_stats(&self) -> Box<dyn Iterator<Item=Result<TripleBatch, StorageError>> + '_> {
        match self { Self::Memory(s) => s.scan_for_stats(), Self::Disk(s) => s.scan_for_stats() }
    }
    fn count(&self, p: Perm, prefix: &[TermId]) -> Result<u64, StorageError> {
        match self { Self::Memory(s) => s.count(p,prefix), Self::Disk(s) => s.count(p,prefix) }
    }
    fn seek(&self, p: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> {
        match self { Self::Memory(s) => s.seek(p,prefix,target), Self::Disk(s) => s.seek(p,prefix,target) }
    }
    fn matching_rows(&self, p: Perm, prefix: &[TermId]) -> Option<&[[TermId; 3]]> {
        match self { Self::Memory(s) => s.matching_rows(p, prefix), Self::Disk(s) => s.matching_rows(p, prefix) }
    }
    fn matching_tail(&self, p: Perm, prefix: &[TermId]) -> Option<&[[TermId; 2]]> {
        match self { Self::Memory(s) => s.matching_tail(p, prefix), Self::Disk(s) => s.matching_tail(p, prefix) }
    }
}
use std::time::Instant;

#[cfg(feature = "server")]
mod server;

pub(crate) fn load(path: &str, args: &[String]) -> Backend {
    let t = Instant::now();
    if args.iter().any(|a| a == "--disk") {
        let snap = DiskSnapshot::open(path).expect("open disk snapshot");
        eprintln!("opened disk snapshot {path} in {:.2}s", t.elapsed().as_secs_f64());
        return Backend::Disk(snap);
    }
    let configured = args.iter().find_map(|arg| arg.strip_prefix("--parallel-load="));
    let workers = configured.map(|v| v.parse::<usize>().expect("parallel-load requires a nonzero integer"))
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()));
    if workers == 0 { eprintln!("parallel-load requires a nonzero integer"); std::process::exit(2); }
    if path.ends_with(".ttl") && configured.is_some() && workers > 1 {
        eprintln!("parallel loader supports N-Triples only"); std::process::exit(2);
    }
    let s = if !path.ends_with(".ttl") && workers > 1 {
        MemorySnapshot::from_ntriples_parallel_file(path, workers).expect("parallel parse")
    } else {
        let f = std::io::BufReader::new(std::fs::File::open(path).expect("open data file"));
        if path.ends_with(".ttl") { MemorySnapshot::from_turtle(f) } else { MemorySnapshot::from_ntriples(f) }.expect("parse")
    };
    eprintln!("loaded {path} in {:.2}s", t.elapsed().as_secs_f64());
    Backend::Memory(s)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    for (i, arg) in args.iter().enumerate() {
        if arg == "--build-external" { triplelight::storage::external_build::force_external_build(); }
        let value=arg.strip_prefix("--build-memory-mb=").or_else(|| (arg == "--build-memory-mb").then(|| args.get(i+1).map(String::as_str)).flatten());
        if arg == "--build-memory-mb" && value.is_none() { eprintln!("--build-memory-mb requires a positive integer"); std::process::exit(2); }
        if let Some(value)=value { let n=value.parse::<usize>().ok().filter(|n|*n>0).unwrap_or_else(|| { eprintln!("--build-memory-mb requires a positive integer"); std::process::exit(2) }); triplelight::storage::external_build::set_build_budget_mb(n).expect("build budget"); }
    }
    for (i, arg) in args.iter().enumerate() {
        let value = arg.strip_prefix("--cache-mb=").or_else(|| (arg == "--cache-mb").then(|| args.get(i + 1).map(String::as_str)).flatten());
        if arg == "--cache-mb" && value.is_none() { eprintln!("--cache-mb requires a positive integer"); std::process::exit(2); }
        if let Some(value) = value {
            let parsed = value.parse::<usize>().ok().filter(|n| *n > 0).unwrap_or_else(|| { eprintln!("--cache-mb requires a positive integer"); std::process::exit(2) });
            triplelight::storage::disk_index::set_cache_budget_mb(parsed).expect("cache budget");
        }
    }

    match args.get(1).map(String::as_str) {
        Some("verify") => {
            if args.len() != 3 { eprintln!("usage: triplelight verify <directory>"); std::process::exit(2); }
            let result=DiskSnapshot::open(&args[2]).and_then(|s|s.verify());
            if let Err(e)=result { eprintln!("verification failed: {e}"); std::process::exit(1); }
            println!("snapshot verified");
        }
        Some("build-disk") => {
            if args.iter().skip(2).any(|arg| arg == "--help" || arg == "-h") {
                println!("usage: triplelight build-disk <source.nt|source.ttl> <new-directory> [--parallel-load=N] [--build-memory-mb=N] [--build-external]\n\
                    --parallel-load=N  Ingestion workers; defaults to available cores, 1 forces serial.\n\
                    External N-Triples parsing uses at most 8 workers and 3 index builders.\n\
                    --build-memory-mb=N  Advisory strategy/run-buffer target, not a process RSS limit.\n\
                    Temporary files, caches, allocator overhead and concurrent work add memory.\n\
                    --build-external  Force the scratch-backed N-Triples builder.");
                return;
            }
            if args.len() < 4 || args.iter().any(|a| a == "--disk") {
                eprintln!("usage: triplelight build-disk <source.nt|source.ttl> <new-directory> [--parallel-load=N]");
                std::process::exit(2);
            }
            if args[2].ends_with(".ttl") {
                let Backend::Memory(source) = load(&args[2], &args) else { unreachable!() };
                DiskSnapshot::create(&args[3], &source).expect("create disk snapshot");
            } else {
                let workers = args.iter().find_map(|arg| arg.strip_prefix("--parallel-load="))
                    .map(|v| v.parse::<usize>().expect("parallel-load requires a nonzero integer"))
                    .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()));
                triplelight::storage::build_disk_snapshot_with_workers(&args[3], Some(std::path::Path::new(&args[2])), workers).expect("create disk snapshot");
            }
        }
        Some("query") => {
            let snap = load(&args[2], &args);
            let t = Instant::now();
            match query(&snap, &args[3]) {
                Ok(r) => { println!("{}", serialize(&r).1); eprintln!("query {:.3} ms", t.elapsed().as_secs_f64() * 1e3) }
                Err(e) => { eprintln!("error: {e}"); std::process::exit(1) }
            }
        }
        Some("stress-commit") => {
            // Test helper: commit one triple per transaction and report each acknowledgement.
            let store = triplelight::store::Store::open(&args[2], None, 1).expect("open store");
            let n: u64 = args[3].parse().unwrap();
            for i in 0..n {
                let t = oxrdf::Triple::new(oxrdf::NamedNode::new_unchecked(format!("http://ex/{i}")),
                    oxrdf::NamedNode::new_unchecked("http://ex/100"), oxrdf::NamedNode::new_unchecked(format!("http://ex/{}", i + 1)));
                store.commit(&[(triplelight::store::Change::Insert, t)]).expect("commit");
                println!("acked {i}");
            }
        }
        Some(cmd @ ("serve" | "serve-store")) => {
            #[cfg(feature = "server")]
            server::run(cmd, &args);
            #[cfg(not(feature = "server"))]
            {
                eprintln!("`{cmd}` needs the HTTP server, which this build does not include. Rebuild with `cargo build --release --features server`.");
                std::process::exit(2);
            }
        }
        _ => eprintln!("usage: triplelight query <data> <sparql> | triplelight build-disk <source> <dir> | triplelight verify <dir>\n\
            with --features server: triplelight serve <data.nt|.ttl> [port] | triplelight serve-store <dir> [port] [--init data.nt] [--read-only]"),
    }
}
