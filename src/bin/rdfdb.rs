//! `rdfdb serve <data.nt|data.ttl> [port]` — load into memory and serve the SPARQL 1.1 protocol at /sparql.
//! `rdfdb query <data> <query>` — run one query and print JSON results.
use rdfdb::exec::{query, results::serialize};
use rdfdb::storage::MemorySnapshot;
use std::io::Read;
use std::time::Instant;

fn load(path: &str) -> MemorySnapshot {
    let t = Instant::now();
    let f = std::io::BufReader::new(std::fs::File::open(path).expect("open data file"));
    let s = if path.ends_with(".ttl") { MemorySnapshot::from_turtle(f) } else { MemorySnapshot::from_ntriples(f) }.expect("parse");
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
            let snap = load(&args[2]);
            let t = Instant::now();
            match query(&snap, &args[3]) {
                Ok(r) => { println!("{}", serialize(&r).1); eprintln!("query {:.3} ms", t.elapsed().as_secs_f64() * 1e3) }
                Err(e) => { eprintln!("error: {e}"); std::process::exit(1) }
            }
        }
        Some("serve") => {
            let snap = load(&args[2]);
            let port = args.get(3).map(String::as_str).unwrap_or("7878");
            let server = tiny_http::Server::http(format!("127.0.0.1:{port}")).expect("bind");
            eprintln!("serving SPARQL at http://127.0.0.1:{port}/sparql");
            for mut req in server.incoming_requests() {
                let url = req.url().to_string();
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                let is_query_body = req.headers().iter().any(|h| h.field.equiv("Content-Type") && h.value.as_str().starts_with("application/sparql-query"));
                let q = if is_query_body { Some(body) } else {
                    url.split_once('?').and_then(|(_, qs)| form_param(qs, "query")).or_else(|| form_param(&body, "query"))
                };
                let resp = match q.map(|q| (std::panic::catch_unwind(|| query(&snap, &q)), q)) {
                    Some((Ok(Ok(r)), _)) => {
                        let (ct, b) = serialize(&r);
                        tiny_http::Response::from_string(b).with_header(tiny_http::Header::from_bytes("Content-Type", ct).unwrap())
                    }
                    Some((Ok(Err(e)), _)) => tiny_http::Response::from_string(e.to_string()).with_status_code(400),
                    Some((Err(_), _)) => tiny_http::Response::from_string("unsupported query feature").with_status_code(501),
                    None => tiny_http::Response::from_string("missing query").with_status_code(400),
                };
                let _ = req.respond(resp);
            }
        }
        _ => eprintln!("usage: rdfdb serve <data.nt|.ttl> [port] | rdfdb query <data> <sparql>"),
    }
}
