//! `rdfdb serve <data.nt|data.ttl> [port]` — load into memory and serve the SPARQL 1.1 protocol at /sparql.
//! `rdfdb query <data> <query>` — run one query and print JSON results.
use rdfdb::exec::{query, query_with, Options, QueryResult, results::{serialize, serialize_ids}};
use rdfdb::opt::stats::Stats;
use rdfdb::storage::MemorySnapshot;
use std::time::Instant;

fn load(path: &str, args: &[String]) -> MemorySnapshot {
    let t = Instant::now();
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
    s
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("query") => {
            let snap = load(&args[2], &args);
            let t = Instant::now();
            match query(&snap, &args[3]) {
                Ok(r) => { println!("{}", serialize(&r).1); eprintln!("query {:.3} ms", t.elapsed().as_secs_f64() * 1e3) }
                Err(e) => { eprintln!("error: {e}"); std::process::exit(1) }
            }
        }
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
                let resp = match q.map(|q| (std::panic::catch_unwind(|| query_with(snap, &q, opts)), q)) {
                    Some((Ok(Ok(r)), _)) => {
                        let (ct, b) = match &r {
                            QueryResult::Ids(sol) => ("application/sparql-results+json", serialize_ids(sol, snap)),
                            r => serialize(r),
                        };
                        tiny_http::Response::from_string(b).with_header(tiny_http::Header::from_bytes("Content-Type", ct).unwrap())
                    }
                    Some((Ok(Err(e)), _)) => tiny_http::Response::from_string(e.to_string()).with_status_code(400),
                    Some((Err(e), _)) => {
                        // Panics: unsupported features (501) or storage failures (500); never an empty result.
                        let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                        let code = if msg.contains("not supported") { 501 } else { 500 };
                        tiny_http::Response::from_string(msg).with_status_code(code)
                    }
                    None => tiny_http::Response::from_string("missing query").with_status_code(400),
                };
                let _ = req.respond(resp);
            }
            }); });
        }
        _ => eprintln!("usage: rdfdb serve <data.nt|.ttl> [port] | rdfdb query <data> <sparql>"),
    }
}
