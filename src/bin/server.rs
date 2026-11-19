//! The optional SPARQL 1.1 HTTP server (`--features server`). TripleLight is an embedded library first; this module
//! is an adapter over the same query API, compiled only when the feature is enabled.
use crate::load;
use std::io::Read;
use std::time::Instant;
use triplelight::exec::{query_with, Options, QueryErrorKind, QueryResult, results::{serialize, serialize_ids}};
use triplelight::opt::stats::Stats;

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
fn answer<S: triplelight::storage::Snapshot>(snap: &S, q: &str, opts: &Options<'_>) -> Resp {
    let before = triplelight::storage::disk_index::cache_trace_enabled().then(triplelight::storage::disk_index::cache_trace_snapshot);
    let response = (|| { match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| query_with(snap, q, opts))) {
        Ok(Ok(r)) => {
            let (ct, b) = match &r {
                QueryResult::Ids(sol) => match serialize_ids(sol, snap) {
                    Ok(b) => ("application/sparql-results+json", b),
                    Err(e) => return tiny_http::Response::from_string(format!("storage error: {e}")).with_status_code(500),
                },
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
    } })();
    if let Some(before) = before {
        let after = triplelight::storage::disk_index::cache_trace_snapshot();
        for (i,name) in ["spo","sop","pso","pos","osp","ops","dictionary.term","dictionary.id"].iter().enumerate() {
            let d: Vec<u64> = (0..5).map(|j|after[i][j].saturating_sub(before[i][j])).collect();
            eprintln!("block_cache {name} hits={} misses={} pread_calls={} pread_bytes={} evictions={}",d[0],d[1],d[2],d[3],d[4]);
        }
    }
    response
}

/// `rdfdb serve-store <dir> [port] [--init data.nt]`: persistent store. Queries at /sparql, updates at /update
/// (SPARQL 1.1 protocol: `update=` form field or `application/sparql-update` body). An update is acknowledged only
/// after its WAL record is fsynced.
fn serve_store(args: &[String], auth: &UpdateAuth) {
    let init = args.iter().position(|a| a == "--init").map(|i| std::path::PathBuf::from(&args[i + 1]));
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let t = Instant::now();
    let store = if args.iter().any(|a| a == "--disk") {
        triplelight::store::Store::open_with(&args[2], init.as_deref(), Box::new(triplelight::storage::DiskBase { workers }))
    } else {
        triplelight::store::Store::open(&args[2], init.as_deref(), workers)
    }.expect("open store");
    eprintln!("opened {} in {:.2}s ({} pending changes replayed)", args[2], t.elapsed().as_secs_f64(), store.snapshot().delta_len());
    // Statistics describe the base; they are used only while there are no pending changes (they could otherwise
    // report a predicate as absent that an update added). Compaction rebuilds them.
    let stats: std::sync::RwLock<Option<(usize, std::sync::Arc<Stats>)>> = std::sync::RwLock::new(None);
    let refresh = |stats: &std::sync::RwLock<Option<(usize, std::sync::Arc<Stats>)>>, snap: &triplelight::store::Overlay| {
        if args.iter().any(|a| a == "--no-stats") { return }
        let st = Stats::from_snapshot(snap.base()).expect("statistics");
        *stats.write().unwrap() = Some((snap.base_id(), std::sync::Arc::new(st)));
    };
    refresh(&stats, &store.snapshot());
    let port = args.get(3).filter(|a| !a.starts_with("--")).map(String::as_str).unwrap_or("7878");
    let server = tiny_http::Server::http(format!("{}:{port}", bind_host())).expect("bind");
    eprintln!("serving SPARQL at http://127.0.0.1:{port}/sparql and /update");
    let (server, store, stats, refresh) = (&server, &store, &stats, &refresh);
    std::thread::scope(|scope| for _ in 0..workers { scope.spawn(move || {
        for mut req in server.incoming_requests() {
            let url = req.url().to_string();
            let path = url.split('?').next().unwrap_or("");
            // Authorize before reading the body, so an unauthorized client cannot make the server buffer it.
            if path == "/update" { if let Err(denied) = auth.check(&req) { let _ = req.respond(denied); continue } }
            let limit = if path == "/update" { u64::MAX } else { MAX_QUERY_BYTES };
            let mut body = String::new();
            if req.as_reader().take(limit.saturating_add(1)).read_to_string(&mut body).is_err() {
                let _ = req.respond(tiny_http::Response::from_string("request body is not valid UTF-8").with_status_code(400)); continue
            }
            if body.len() as u64 > limit {
                let _ = req.respond(tiny_http::Response::from_string("query too large").with_status_code(413)); continue
            }
            let ctype = req.headers().iter().find(|h| h.field.equiv("Content-Type")).map(|h| h.value.as_str().to_string()).unwrap_or_default();
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

/// Listen address host: 127.0.0.1 unless TRIPLELIGHT_HOST (or the older RDFDB_HOST) is set, e.g. 0.0.0.0 in a container.
fn bind_host() -> String {
    std::env::var("TRIPLELIGHT_HOST").or_else(|_| std::env::var("RDFDB_HOST")).unwrap_or_else(|_| "127.0.0.1".into())
}

fn is_loopback(host: &str) -> bool {
    host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Largest query body accepted (updates, which are authorized first, are not limited).
const MAX_QUERY_BYTES: u64 = 16 << 20;

/// Who may call /update on `serve-store`.
///
/// - `TRIPLELIGHT_UPDATE_TOKEN` set: requests must send `Authorization: Bearer <token>` (401 otherwise).
/// - `--allow-unauthenticated-updates`: anyone who can reach the port, except requests carrying an `Origin` header
///   (a browser page posting a cross-origin form to localhost). Refused on a non-loopback address.
/// - otherwise, including `--read-only`: nobody (403). Loopback alone is not an authentication boundary: any web
///   page in a local browser can POST to 127.0.0.1, so open updates are never the default.
pub(crate) enum UpdateAuth { ReadOnly, Token(String), Open }

impl UpdateAuth {
    fn from_args(args: &[String], host: &str) -> Result<UpdateAuth, String> {
        if args.iter().any(|a| a == "--read-only") { return Ok(UpdateAuth::ReadOnly) }
        if let Some(t) = std::env::var("TRIPLELIGHT_UPDATE_TOKEN").ok().filter(|t| !t.is_empty()) { return Ok(UpdateAuth::Token(t)) }
        if args.iter().any(|a| a == "--allow-unauthenticated-updates") {
            if is_loopback(host) { return Ok(UpdateAuth::Open) }
            return Err(format!("refusing unauthenticated updates on {host}: set TRIPLELIGHT_UPDATE_TOKEN or pass --read-only"))
        }
        Ok(UpdateAuth::ReadOnly)
    }

    fn check(&self, req: &tiny_http::Request) -> Result<(), Resp> {
        match self {
            UpdateAuth::Open if req.headers().iter().any(|h| h.field.equiv("Origin")) =>
                Err(tiny_http::Response::from_string("updates from browser pages are refused").with_status_code(403)),
            UpdateAuth::Open => Ok(()),
            UpdateAuth::ReadOnly => Err(tiny_http::Response::from_string(
                "updates are disabled: start the server with TRIPLELIGHT_UPDATE_TOKEN set").with_status_code(403)),
            UpdateAuth::Token(t) => {
                let sent = req.headers().iter().find(|h| h.field.equiv("Authorization")).map(|h| h.value.as_str().to_string());
                let ok = sent.as_deref().and_then(|v| v.strip_prefix("Bearer ")).is_some_and(|v| constant_time_eq(v.as_bytes(), t.as_bytes()));
                if ok { Ok(()) } else {
                    Err(tiny_http::Response::from_string("missing or wrong update token").with_status_code(401)
                        .with_header(tiny_http::Header::from_bytes("WWW-Authenticate", "Bearer").unwrap()))
                }
            }
        }
    }
}

/// Compares without an early exit on the first differing byte (the length is not secret).
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Entry point for `serve` (in-memory, read-only) and `serve-store` (persistent, with /update).
pub(crate) fn run(cmd: &str, args: &[String]) {
    if cmd == "serve-store" {
        let auth = UpdateAuth::from_args(args, &bind_host()).unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(2) });
        eprintln!("updates: {}", match &auth { UpdateAuth::ReadOnly => "disabled (set TRIPLELIGHT_UPDATE_TOKEN to enable)", UpdateAuth::Token(_) => "require TRIPLELIGHT_UPDATE_TOKEN", UpdateAuth::Open => "open to local clients without a browser Origin (--allow-unauthenticated-updates)" });
        return serve_store(args, &auth);
    }
    serve_memory(args);
}

/// `triplelight serve <data> [port]`: load a file into memory and answer queries (no updates).
fn serve_memory(args: &[String]) {
            let snap = load(&args[2], &args);
            let port = args.get(3).map(String::as_str).unwrap_or("7878");
            // Optimizer statistics (on by default, --no-stats disables) are built on a background thread so the
            // server answers immediately: until they are ready, joins use the greedy order. Join and leapfrog orders
            // then come from the cost model, which a work gate skips for small queries (round-2 benchmarks).
            let stats: std::sync::RwLock<Option<std::sync::Arc<Stats>>> = std::sync::RwLock::new(None);
            let want_stats = !args.iter().any(|a| a == "--no-stats");
            eprintln!("optimizer statistics: {}", if want_stats { "building in background" } else { "off" });
            let server = tiny_http::Server::http(format!("{}:{port}", bind_host())).expect("bind");
            eprintln!("serving SPARQL at http://127.0.0.1:{port}/sparql");
            // One worker per core: queries run concurrently against the same immutable snapshot.
            let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
            let (server, snap, stats) = (&server, &snap, &stats);
            std::thread::scope(|scope| {
            if want_stats { scope.spawn(move || {
                let t = Instant::now();
                match Stats::from_snapshot(snap) {
                    Ok(st) => { *stats.write().unwrap() = Some(std::sync::Arc::new(st)); eprintln!("statistics ready after {:.2}s", t.elapsed().as_secs_f64()) }
                    Err(e) => eprintln!("statistics failed ({e}); continuing with greedy join order"),
                }
            }); }
            for _ in 0..workers { scope.spawn(move || {
            for mut req in server.incoming_requests() {
                let url = req.url().to_string();
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                let is_query_body = req.headers().iter().any(|h| h.field.equiv("Content-Type") && h.value.as_str().starts_with("application/sparql-query"));
                let q = if is_query_body { Some(body) } else {
                    url.split_once('?').and_then(|(_, qs)| form_param(qs, "query")).or_else(|| form_param(&body, "query"))
                };
                let st = stats.read().unwrap().clone();
                let opts = Options { stats: st.as_deref(), wcoj: true, raw_ids: true };
                let resp = match q { Some(q) => answer(snap, &q, &opts), None => tiny_http::Response::from_string("missing query").with_status_code(400) };
                let _ = req.respond(resp);
            }
            }); } });
}

