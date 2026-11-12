//! `rdfdb serve <data.nt|data.ttl> [port]` — load into memory and serve the SPARQL 1.1 protocol at /sparql.
//! `rdfdb query <data> <query>` — run one query and print JSON results.
use rdfdb::exec::{query, query_with, Options, QueryErrorKind, QueryResult, results::{serialize, serialize_ids}};
use rdfdb::opt::stats::Stats;
use rdfdb::storage::{MemorySnapshot, DiskSnapshot, Dictionary, Snapshot, StorageError};
use rdfdb::model::{Perm, TermId, TripleBatch};
use oxrdf::{Term, TermRef};

enum Backend { Memory(MemorySnapshot), Disk(DiskSnapshot) }
impl Dictionary for Backend {
    fn lookup(&self, term: &Term) -> Option<TermId> { match self { Self::Memory(s) => s.lookup(term), Self::Disk(s) => s.lookup(term) } }
    fn resolve(&self, id: TermId) -> Term { match self { Self::Memory(s) => s.resolve(id), Self::Disk(s) => s.resolve(id) } }
    fn resolve_ref(&self, id: TermId) -> TermRef<'_> { match self { Self::Memory(s) => s.resolve_ref(id), Self::Disk(s) => s.resolve_ref(id) } }
}
impl Snapshot for Backend {
    fn scan<'a>(&'a self, p: Perm, prefix: &[TermId]) -> Box<dyn Iterator<Item=Result<TripleBatch, StorageError>> + 'a> {
        match self { Self::Memory(s) => s.scan(p,prefix), Self::Disk(s) => s.scan(p,prefix) }
    }
    fn count(&self, p: Perm, prefix: &[TermId]) -> Result<u64, StorageError> {
        match self { Self::Memory(s) => s.count(p,prefix), Self::Disk(s) => s.count(p,prefix) }
    }
    fn seek(&self, p: Perm, prefix: &[TermId], target: TermId) -> Result<Option<TermId>, StorageError> {
        match self { Self::Memory(s) => s.seek(p,prefix,target), Self::Disk(s) => s.seek(p,prefix,target) }
    }
}
use std::time::Instant;

fn load(path: &str, args: &[String]) -> Backend {
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

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) { out.push(v); i += 2 } else { out.push(b'%') }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn form_param(body: &str, key: &str) -> Option<String> {
    body.split('&').find_map(|kv| { let (k, v) = kv.split_once('=')?; (k == key).then(|| decode(v)) })
}

type Resp = tiny_http::Response<std::io::Cursor<Vec<u8>>>;

fn error_code(kind: QueryErrorKind) -> u16 { match kind { QueryErrorKind::Syntax => 400, QueryErrorKind::Unsupported => 501, QueryErrorKind::Storage => 500 } }

/// One SPARQL query against a snapshot, as an HTTP response. Errors: 400 (bad query), 501 (unsupported), 500 (storage);
/// never an empty result in place of a failure.
fn answer<S: rdfdb::storage::Snapshot>(snap: &S, q: &str, opts: &Options<'_>) -> Resp {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| query_with(snap, q, opts))) {
        Ok(Ok(r)) => {
            let (ct, b) = match &r {
                QueryResult::Ids(sol) => ("application/sparql-results+json", serialize_ids(sol, snap)),
                r => serialize(r),
            };
            tiny_http::Response::from_string(b).with_header(tiny_http::Header::from_bytes("Content-Type", ct).unwrap())
        }
        Ok(Err(e)) => tiny_http::Response::from_string(e.to_string()).with_status_code(error_code(e.kind)),
        Err(e) => {
            let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
            let code = if msg.contains("not supported") { 501 } else { 500 };
            tiny_http::Response::from_string(msg).with_status_code(code)
        }
    }
}

/// `rdfdb serve-store <dir> [port] [--init data.nt]`: persistent store. Queries at /sparql, updates at /update
/// (SPARQL 1.1 protocol: `update=` form field or `application/sparql-update` body). An update is acknowledged only
/// after its WAL record is fsynced.
fn serve_store(args: &[String]) {
    let init = args.iter().position(|a| a == "--init").map(|i| std::path::PathBuf::from(&args[i + 1]));
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let t = Instant::now();
    let store = if args.iter().any(|a| a == "--disk") {
        rdfdb::store::Store::open_with(&args[2], init.as_deref(), Box::new(rdfdb::storage::DiskBase { workers }))
    } else {
        rdfdb::store::Store::open(&args[2], init.as_deref(), workers)
    }.expect("open store");
    eprintln!("opened {} in {:.2}s ({} pending changes replayed)", args[2], t.elapsed().as_secs_f64(), store.snapshot().delta_len());
    // Statistics describe the base; they are used only while there are no pending changes (they could otherwise
    // report a predicate as absent that an update added). Compaction rebuilds them.
    let stats: std::sync::RwLock<Option<(usize, std::sync::Arc<Stats>)>> = std::sync::RwLock::new(None);
    let refresh = |stats: &std::sync::RwLock<Option<(usize, std::sync::Arc<Stats>)>>, snap: &rdfdb::store::Overlay| {
        if args.iter().any(|a| a == "--no-stats") { return }
        let st = Stats::from_snapshot(snap.base()).expect("statistics");
        *stats.write().unwrap() = Some((snap.base_id(), std::sync::Arc::new(st)));
    };
    refresh(&stats, &store.snapshot());
    let port = args.get(3).filter(|a| !a.starts_with("--")).map(String::as_str).unwrap_or("7878");
    let server = tiny_http::Server::http(format!("127.0.0.1:{port}")).expect("bind");
    eprintln!("serving SPARQL at http://127.0.0.1:{port}/sparql and /update");
    let (server, store, stats, refresh) = (&server, &store, &stats, &refresh);
    std::thread::scope(|scope| for _ in 0..workers { scope.spawn(move || {
        for mut req in server.incoming_requests() {
            let url = req.url().to_string();
            let mut body = String::new();
            let _ = req.as_reader().read_to_string(&mut body);
            let ctype = req.headers().iter().find(|h| h.field.equiv("Content-Type")).map(|h| h.value.as_str().to_string()).unwrap_or_default();
            let path = url.split('?').next().unwrap_or("");
            let resp = if path == "/update" {
                let u = if ctype.starts_with("application/sparql-update") { Some(body) } else { form_param(&body, "update") };
                match u {
                    None => tiny_http::Response::from_string("missing update").with_status_code(400),
                    Some(u) => match store.update(&u) {
                        Ok(n) => {
                            let snap = store.snapshot();
                            if snap.delta_len() > 1_000_000 {
                                if let Err(e) = store.compact() { eprintln!("compaction failed: {e}") } else { refresh(stats, &store.snapshot()) }
                            }
                            tiny_http::Response::from_string(format!("{n} triples changed"))
                        }
                        Err(e) => tiny_http::Response::from_string(e.to_string()).with_status_code(error_code(e.kind)),
                    },
                }
            } else {
                let q = if ctype.starts_with("application/sparql-query") { Some(body) } else {
                    url.split_once('?').and_then(|(_, qs)| form_param(qs, "query")).or_else(|| form_param(&body, "query"))
                };
                match q {
                    None => tiny_http::Response::from_string("missing query").with_status_code(400),
                    Some(q) => {
                        let snap = store.snapshot();
                        let st = stats.read().unwrap().clone();
                        let st = st.filter(|(base, _)| snap.delta_len() == 0 && *base == snap.base_id());
                        let opts = Options { stats: st.as_ref().map(|(_, s)| &**s), wcoj: true, raw_ids: true };
                        answer(&*snap, &q, &opts)
                    }
                }
            };
            let _ = req.respond(resp);
        }
    }); });
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("verify") => {
            if args.len() != 3 { eprintln!("usage: rdfdb verify <directory>"); std::process::exit(2); }
            let result=DiskSnapshot::open(&args[2]).and_then(|s|s.verify());
            if let Err(e)=result { eprintln!("verification failed: {e}"); std::process::exit(1); }
            println!("snapshot verified");
        }
        Some("build-disk") => {
            if args.len() < 4 || args.iter().any(|a| a == "--disk") {
                eprintln!("usage: rdfdb build-disk <source.nt|source.ttl> <new-directory> [--parallel-load=N]");
                std::process::exit(2);
            }
            let Backend::Memory(source) = load(&args[2], &args) else { unreachable!() };
            DiskSnapshot::create(&args[3], &source).expect("create disk snapshot");
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
            let store = rdfdb::store::Store::open(&args[2], None, 1).expect("open store");
            let n: u64 = args[3].parse().unwrap();
            for i in 0..n {
                let t = oxrdf::Triple::new(oxrdf::NamedNode::new_unchecked(format!("http://ex/{i}")),
                    oxrdf::NamedNode::new_unchecked("http://ex/100"), oxrdf::NamedNode::new_unchecked(format!("http://ex/{}", i + 1)));
                store.commit(&[(rdfdb::store::Change::Insert, t)]).expect("commit");
                println!("acked {i}");
            }
        }
        Some("serve-store") => serve_store(&args),
        Some("serve") => {
            let snap = load(&args[2], &args);
            let port = args.get(3).map(String::as_str).unwrap_or("7878");
            // Optimizer statistics are built at load by default (--no-stats disables): join and leapfrog orders then come
            // from the cost model, which a work gate skips for small queries. Decided on round-2 benchmarks (bench/results).
            let stats = (!args.iter().any(|a| a == "--no-stats")).then(|| {
                let t = Instant::now();
                let st = Stats::from_snapshot(&snap).expect("statistics");
                eprintln!("statistics built in {:.2}s", t.elapsed().as_secs_f64());
                st
            });
            eprintln!("optimizer statistics: {}", if stats.is_some() { "on" } else { "off" });
            let opts = Options { stats: stats.as_ref(), wcoj: true, raw_ids: true };
            let opts = &opts;
            let server = tiny_http::Server::http(format!("127.0.0.1:{port}")).expect("bind");
            eprintln!("serving SPARQL at http://127.0.0.1:{port}/sparql");
            // One worker per core: queries run concurrently against the same immutable snapshot.
            let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
            let (server, snap) = (&server, &snap);
            std::thread::scope(|scope| for _ in 0..workers { scope.spawn(move || {
            for mut req in server.incoming_requests() {
                let url = req.url().to_string();
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                let is_query_body = req.headers().iter().any(|h| h.field.equiv("Content-Type") && h.value.as_str().starts_with("application/sparql-query"));
                let q = if is_query_body { Some(body) } else {
                    url.split_once('?').and_then(|(_, qs)| form_param(qs, "query")).or_else(|| form_param(&body, "query"))
                };
                let resp = match q { Some(q) => answer(snap, &q, opts), None => tiny_http::Response::from_string("missing query").with_status_code(400) };
                let _ = req.respond(resp);
            }
            }); });
        }
        _ => eprintln!("usage: rdfdb serve <data.nt|.ttl> [port] | rdfdb serve-store <dir> [port] [--init data.nt] | rdfdb query <data> <sparql>"),
    }
}
