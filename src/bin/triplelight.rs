//! `triplelight query <data> <query>` — run one query and print JSON results.
//! With the `server` feature: `triplelight serve` / `serve-store` speak the SPARQL 1.1 protocol (see `server.rs`).
use triplelight::exec::{Entailment, Options, query_with, results::serialize};
use triplelight::storage::{MemorySnapshot, DiskSnapshot, Dictionary, Snapshot, StorageError};
use triplelight::model::{Perm, TermId, TripleBatch};
use triplelight_rdf::{Term, TermRef};

#[cfg(target_os = "linux")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub(crate) enum Backend { Memory(MemorySnapshot), Disk(DiskSnapshot) }
/// Forwards a call to whichever snapshot the CLI opened.
macro_rules! each { ($self:ident, $s:ident => $e:expr) => { match $self { Backend::Memory($s) => $e, Backend::Disk($s) => $e } } }
impl Dictionary for Backend {
    fn lookup(&self, term: &Term) -> Result<Option<TermId>, StorageError> { each!(self, s => s.lookup(term)) }
    fn literal_text_bound(&self, text: &str, strict: bool) -> Result<Option<TermId>, StorageError> { each!(self, s => s.literal_text_bound(text, strict)) }
    fn resolve(&self, id: TermId) -> Result<Term, StorageError> { each!(self, s => s.resolve(id)) }
    fn with_term(&self, id: TermId, f: &mut dyn FnMut(TermRef<'_>)) -> Result<(), StorageError> { each!(self, s => s.with_term(id, f)) }
    fn with_terms(&self, ids: &[TermId], f: &mut dyn FnMut(usize, TermRef<'_>)) -> Result<(), StorageError> { each!(self, s => s.with_terms(ids, f)) }
}
impl Snapshot for Backend {
    fn scan<'a>(&'a self, p: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item=Result<TripleBatch, StorageError>> + 'a> { each!(self, s => s.scan(p, prefix)) }
    fn scan_for_stats(&self) -> Box<dyn Iterator<Item=Result<TripleBatch, StorageError>> + '_> { each!(self, s => s.scan_for_stats()) }
    fn scan_from<'a>(&'a self, p: Perm, prefix: &[TermId], from: TermId) -> Box<dyn Iterator<Item=Result<TripleBatch, StorageError>> + 'a> { each!(self, s => s.scan_from(p, prefix, from)) }
    fn first_keys(&self, p: Perm) -> Result<Vec<TermId>, StorageError> { each!(self, s => s.first_keys(p)) }
    fn count(&self, p: Perm, prefix: &[TermId]) -> Result<u64, StorageError> { each!(self, s => s.count(p, prefix)) }
    fn seek(&self, p: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> { each!(self, s => s.seek(p, prefix, target)) }
    fn matching_rows(&self, p: Perm, prefix: &[TermId]) -> Option<&[[TermId; 3]]> { each!(self, s => s.matching_rows(p, prefix)) }
    fn matching_tail(&self, p: Perm, prefix: &[TermId]) -> Option<&[[TermId; 2]]> { each!(self, s => s.matching_tail(p, prefix)) }
    fn object_memo(&self) -> Option<&triplelight::storage::ObjectMemo> { each!(self, s => s.object_memo()) }
}
use std::time::Instant;

#[cfg(feature = "server")]
mod server;
#[cfg(unix)]
#[path = "shell/mod.rs"]
mod shell;

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

/// `--entailment=simple|rdfs` (default simple): the regime `query` matches graph patterns under (#108).
pub(crate) fn entailment_arg(args: &[String]) -> Entailment {
    match args.iter().find_map(|a| a.strip_prefix("--entailment=")) {
        None | Some("simple") => Entailment::Simple,
        Some("rdfs") => Entailment::Rdfs,
        Some(other) => { eprintln!("--entailment must be simple or rdfs, not {other}"); std::process::exit(2) }
    }
}

fn print_query<E: std::fmt::Display>(query: impl FnOnce() -> Result<triplelight::exec::QueryResult, E>) {
    let t = Instant::now();
    match query() {
        Ok(result) => { println!("{}", serialize(&result).1); eprintln!("query {:.3} ms", t.elapsed().as_secs_f64() * 1e3) }
        Err(e) => { eprintln!("error: {e}"); std::process::exit(1) }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    #[cfg(unix)]
    match args.get(1).map(String::as_str) {
        None => return shell::run(&[]),
        Some("shell") => return shell::run(&args[2..]),
        Some("-c" | "--command" | "--mode" | ":memory:") => return shell::run(&args[1..]),
        Some(path) if !path.starts_with('-') && path.contains('/') => return shell::run(&args[1..]),
        _ => {}
    }
    for (i, arg) in args.iter().enumerate() {
        if arg == "--build-external" { triplelight::storage::external_build::force_external_build(); }
        // Task #73: `--orders=4` (the default) keeps SPO, PSO, POS and OSP (an RDFSNAP6 base, external builder only);
        // SOP and OPS are then served from them. `--orders=6` keeps all six (RDFSNAP4/5, readable by older binaries).
        if let Some(value) = arg.strip_prefix("--orders=") {
            use triplelight::storage::{ALL_ORDERS, REQUIRED_ORDERS, external_build};
            let orders = match value { "4" => REQUIRED_ORDERS, "6" => ALL_ORDERS, _ => { eprintln!("--orders must be 4 or 6"); std::process::exit(2) } };
            external_build::set_kept_orders(orders).expect("order set");
            if orders != ALL_ORDERS { external_build::force_external_build() }
        }
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
            // File sizes, so a receipt shows what each order and the dictionary cost (task #73).
            let mut files: Vec<_> = std::fs::read_dir(&args[2]).expect("read snapshot directory").filter_map(Result::ok)
                .filter_map(|e| Some((e.file_name().into_string().ok()?, e.metadata().ok()?.len()))).collect();
            files.sort();
            for (name, bytes) in files { println!("{name} {bytes} bytes") }
        }
        Some("build-disk") => {
            if args.iter().skip(2).any(|arg| arg == "--help" || arg == "-h") {
                println!("usage: triplelight build-disk <source.nt|source.ttl> <new-directory> [--parallel-load=N] [--build-memory-mb=N] [--build-external] [--orders=4|6]\n\
                    --parallel-load=N  Ingestion workers; defaults to available cores, 1 forces serial.\n\
                    External N-Triples parsing uses at most 8 workers and 3 index builders.\n\
                    --build-memory-mb=N  Advisory strategy/run-buffer target, not a process RSS limit.\n\
                    Temporary files, caches, allocator overhead and concurrent work add memory.\n\
                    --build-external  Force the scratch-backed N-Triples builder.\n\
                    --orders=4|6  Index orders to build. 4 (default, N-Triples only) keeps SPO, PSO, POS and OSP and serves\n\
                    SOP and OPS from them; 6 keeps all six, a layout older versions can open. Turtle always builds six.");
                return;
            }
            if args.len() < 4 || args.iter().any(|a| a == "--disk") {
                eprintln!("usage: triplelight build-disk <source.nt|source.ttl> <new-directory> [--parallel-load=N]");
                std::process::exit(2);
            }
            if args[2].ends_with(".ttl") {
                // Turtle takes the in-memory path, which writes all six orders, so a smaller order set can't be honoured.
                if args.iter().any(|a| a.starts_with("--orders=") && a != "--orders=6") {
                    eprintln!("--orders=4 needs N-Triples input; Turtle builds keep all six orders");
                    std::process::exit(2);
                }
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
            let path = &args[2];
            let opts = Options { entailment: entailment_arg(&args), ..Default::default() };
            #[cfg(unix)]
            if !args.iter().any(|a| a == "--disk") && (path.ends_with(".trig") || path.ends_with(".nq")) {
                let db = triplelight::embedded::Database::in_memory();
                if let Err(e) = db.load(path) {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
                print_query(|| db.query_with_entailment(&args[3], opts.entailment));
                return;
            }
            let snap = load(path, &args);
            print_query(|| query_with(&snap, &args[3], &opts));
        }
        Some("text-index") => {
            let usage = || -> ! {
                eprintln!("usage: triplelight text-index <store-directory> [--build-memory-mb=N]");
                std::process::exit(2)
            };
            let [_, _, directory, rest @ ..] = args.as_slice() else { usage(); };
            let value = match rest {
                [] => "128",
                [arg] => arg.strip_prefix("--build-memory-mb=").unwrap_or_else(|| usage()),
                [flag, value] if flag == "--build-memory-mb" => value,
                _ => usage(),
            };
            let memory_mb = value.parse::<usize>().unwrap_or_else(|_| usage());
            let directory = std::path::Path::new(directory);
            if !directory.join("CURRENT").is_file() {
                eprintln!("text-index requires an existing disk store");
                std::process::exit(2);
            }
            let result = triplelight::store::Store::open_with(
                directory, None, Box::new(triplelight::storage::DiskBase { workers: 1 }),
            ).and_then(|store| DiskSnapshot::build_text_index(&store, memory_mb));
            match result {
                Ok(tokens) => println!("indexed {tokens} distinct tokens"),
                Err(e) => { eprintln!("text index build failed: {e}"); std::process::exit(1); }
            }
        }
        Some("explain") => {
            let [_, _, data, query, ..] = args.as_slice() else {
                eprintln!("usage: triplelight explain <data> <sparql> [--entailment=simple|rdfs]");
                std::process::exit(2);
            };
            let snap = load(data, &args);
            let opts = Options { entailment: entailment_arg(&args), ..Default::default() };
            match triplelight::exec::plan_trace::explain(&snap, query, &opts) {
                Ok(report) => print!("{report}"),
                Err(e) => { eprintln!("error: {e}"); std::process::exit(1) }
            }
        }
        #[cfg(feature = "shacl")]
        Some("validate") => {
            // SHACL Core (#108). Exit status: 0 the data conforms, 1 it does not (the report says why), 2 usage or I/O,
            // 3 a syntax error in either document, 4 shapes SHACL rejects or this build does not support.
            let [data, shapes] = [args.get(2), args.get(3)].map(|p| p.cloned().unwrap_or_else(|| {
                eprintln!("usage: triplelight validate <data.nt|.ttl> <shapes.ttl|.nt>"); std::process::exit(2)
            }));
            let read = |path: &str| std::fs::read(path).unwrap_or_else(|e| { eprintln!("{path}: {e}"); std::process::exit(2) });
            let format = |path: &str| if path.ends_with(".nt") { triplelight::rdf::Format::NTriples } else { triplelight::rdf::Format::Turtle };
            match triplelight::shacl::validate_documents(&read(&data), format(&data), &read(&shapes), format(&shapes)) {
                Ok(report) => {
                    for t in report.to_triples() { println!("{t} .") }
                    std::process::exit(if report.conforms { 0 } else { 1 })
                }
                Err(e @ triplelight::shacl::DocumentError::Syntax { .. }) => { eprintln!("error: {e}"); std::process::exit(3) }
                Err(e) => { eprintln!("error: {e}"); std::process::exit(4) }
            }
        }
        #[cfg(not(feature = "shacl"))]
        Some("validate") => {
            eprintln!("error: SHACL validation requires the shacl Cargo feature");
            std::process::exit(4);
        }
        Some("stress-commit") => {
            // Test helper: commit one triple per transaction and report each acknowledgement.
            let store = triplelight::store::Store::open(&args[2], None, 1).expect("open store");
            let n: u64 = args[3].parse().unwrap();
            for i in 0..n {
                let t = triplelight_rdf::Triple::new(triplelight_rdf::NamedNode::new_unchecked(format!("http://ex/{i}")),
                    triplelight_rdf::NamedNode::new_unchecked("http://ex/100"), triplelight_rdf::NamedNode::new_unchecked(format!("http://ex/{}", i + 1)));
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
        _ => eprintln!("usage: triplelight shell [store-directory] [-c SPARQL] (Unix) | triplelight query <data> <sparql> [--entailment=simple|rdfs] | triplelight explain <data> <sparql> | triplelight validate <data> <shapes> | triplelight build-disk <source> <dir> | triplelight verify <dir> | triplelight text-index <store-directory>\n\
            with --features server: triplelight serve <data.nt|.ttl> [port] | triplelight serve-store <dir> [port] [--init data.nt] [--read-only]"),
    }
}
