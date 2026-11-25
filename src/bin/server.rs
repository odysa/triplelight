//! The optional SPARQL 1.1 HTTP server (`--features server`). TripleLight is an embedded library first; this module
//! is an adapter over the same query API, compiled only when the feature is enabled.
use crate::load;
use std::io::Read;
use std::time::Instant;
use triplelight::exec::{query_with, Entailment, Interrupt, MemoryBudget, Options, QueryErrorKind, QueryResult, results::{serialize, serialize_ids_v, serialize_csv, serialize_tsv, IdsJsonStream}};
use triplelight::opt::stats::Stats;
use triplelight::store::StoreUpdate;

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                if let Some(v) = s.get(i + 1..i + 3).and_then(|hex| u8::from_str_radix(hex, 16).ok()) { out.push(v); i += 2 } else { out.push(b'%') }
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
/// A request's entailment regime (#108): `entailment=simple|rdfs` in the URL or the form body, else the server's
/// `--entailment=` default. A SPARQL query body has no form fields, so only the URL counts then.
fn request_entailment(url: &str, form: Option<&str>, default: Entailment) -> Result<Entailment, String> {
    let named = url.split_once('?').and_then(|(_, qs)| form_param(qs, "entailment")).or_else(|| form.and_then(|b| form_param(b, "entailment")));
    match named.as_deref() {
        None => Ok(default),
        Some("simple") => Ok(Entailment::Simple),
        Some("rdfs") => Ok(Entailment::Rdfs),
        Some(other) => Err(format!("entailment must be simple or rdfs, not {other:?}")),
    }
}

/// `--query-timeout=<seconds>`: each query request stops with HTTP 503 once it has run that long (#110). None: no limit.
fn query_timeout(args: &[String]) -> Option<std::time::Duration> {
    let v = args.iter().find_map(|a| a.strip_prefix("--query-timeout="))?;
    match v.parse::<f64>() {
        Ok(s) if s > 0.0 && s.is_finite() => Some(std::time::Duration::from_secs_f64(s)),
        _ => { eprintln!("--query-timeout must be a positive number of seconds, not {v:?}"); std::process::exit(2) }
    }
}

/// `--query-memory-mb=<n>`: each query request gets a working-memory budget of that size and fails with HTTP 503 instead
/// of growing past it (#110). None: unbounded. Under a budget, a GROUP BY whose input does not fit spills it to
/// `TRIPLELIGHT_SCRATCH_DIR` (else the system temporary directory) instead of failing (#111); `spill_bytes=` on the
/// `TRIPLELIGHT_TRACE_PHASES` line shows how much it wrote. Point the variable at a local disk with room for the largest
/// intermediate result: a tmpfs is charged to memory.
fn query_memory(args: &[String]) -> Option<u64> {
    let v = args.iter().find_map(|a| a.strip_prefix("--query-memory-mb="))?;
    match v.parse::<u64>() {
        Ok(n) if n > 0 => {
            // Spill directories left by a server that was killed mid-query (#111): nobody holds their locks any more.
            let scratch = MemoryBudget::default_scratch_dir();
            match triplelight::exec::sweep_stale_spills(&scratch) {
                Ok(0) => {}
                Ok(n) => eprintln!("removed {n} stale spill directories from {}", scratch.display()),
                Err(e) => eprintln!("could not sweep stale spill directories in {}: {e}", scratch.display()),
            }
            Some(n << 20)
        }
        _ => { eprintln!("--query-memory-mb must be a positive integer, not {v:?}"); std::process::exit(2) }
    }
}

/// Result format chosen from the `Accept` header: the first of SPARQL JSON, TSV and CSV that it names (JSON when none).
/// Boolean and graph results have no CSV or TSV form and are always answered in their own format.
#[derive(Clone, Copy, PartialEq)]
enum Format { Json, Tsv, Csv }

fn format_from_accept(accept: &str) -> Format {
    // RFC 9110 content negotiation over the three formats we write: whole media types only (parameters other than `q` are
    // ignored), the highest `q` wins, `q=0` means "not acceptable", ties go to the earlier entry, and anything else
    // (no header, `*/*`, a type we don't write) falls back to SPARQL JSON.
    let mut best: Option<(f32, Format)> = None;
    for item in accept.split(',') {
        let mut parts = item.split(';');
        let media = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        let fmt = match media.as_str() {
            "application/sparql-results+json" | "application/json" => Format::Json,
            "text/tab-separated-values" => Format::Tsv,
            "text/csv" => Format::Csv,
            _ => continue,
        };
        let q = parts.find_map(|p| p.trim().strip_prefix("q=").or_else(|| p.trim().strip_prefix("Q=")).and_then(|v| v.trim().parse::<f32>().ok())).unwrap_or(1.0);
        if q > 0.0 && best.is_none_or(|(bq, _)| q > bq) { best = Some((q, fmt)); }
    }
    best.map_or(Format::Json, |(_, f)| f)
}

fn accept_of(req: &tiny_http::Request) -> Format {
    format_from_accept(&req.headers().iter().find(|h| h.field.equiv("Accept")).map(|h| h.value.as_str().to_string()).unwrap_or_default())
}

type Resp = tiny_http::Response<std::io::Cursor<Vec<u8>>>;

/// An answer to one query: a whole response, or (for SPARQL JSON over id rows) a body that is serialized window by window
/// while it is read, so the first bytes leave before the rest of the rows are decoded and the body is never held whole.
enum Reply<'a> { Whole(Resp), Stream(tiny_http::Response<Box<dyn Read + 'a>>) }

/// The body of a streamed JSON answer. `read` serializes the next window when the previous one is sent; an error ends the
/// body without its closing braces, so the body is invalid JSON: a client sees a truncated answer, never a wrong one. (tiny_http still writes the
/// chunked encoding's final `0` chunk when the reader returns an error, so the missing `]}}` is what protects the client, not a broken framing.)
struct JsonBody<'a, S: triplelight::storage::Snapshot> {
    stream: IdsJsonStream<triplelight::exec::IdSolutions>,
    snap: &'a S,
    buf: Vec<u8>,
    pos: usize,
}

impl<S: triplelight::storage::Snapshot> Read for JsonBody<'_, S> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.pos == self.buf.len() {
            match self.stream.next_piece(self.snap) {
                Ok(Some(piece)) => { self.buf = piece.into_bytes(); self.pos = 0 }
                Ok(None) => return Ok(0),
                Err(e) => { eprintln!("streamed answer failed after its first window: {e}"); return Err(std::io::Error::other(e)) }
            }
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fn error_code(kind: QueryErrorKind) -> u16 { match kind { QueryErrorKind::Syntax => 400, QueryErrorKind::Unsupported => 501, QueryErrorKind::Storage => 500, QueryErrorKind::GraphState => 400, QueryErrorKind::Cancelled | QueryErrorKind::Timeout | QueryErrorKind::ResourceLimit => 503 } }

/// A runtime explanation, requested explicitly with a raw SPARQL query body.
fn explain<S: triplelight::storage::Snapshot>(req: &mut tiny_http::Request, snap: &S, opts: Options<'_>, timeout: Option<std::time::Duration>, memory: Option<u64>) -> Resp {
    let reply = |body, code| tiny_http::Response::from_string(body).with_status_code(code);
    if req.method() != &tiny_http::Method::Post {
        return reply("use POST with a SPARQL query body".to_string(), 405)
            .with_header(tiny_http::Header::from_bytes("Allow", "POST").unwrap());
    }
    if !req.headers().iter().any(|h| h.field.equiv("Content-Type")
        && h.value.as_str().split(';').next().is_some_and(|media| media.trim().eq_ignore_ascii_case("application/sparql-query"))) {
        return reply("expected application/sparql-query".to_string(), 415);
    }
    let entailment = match request_entailment(req.url(), None, opts.entailment) {
        Ok(value) => value,
        Err(e) => return reply(e, 400),
    };
    let mut query = String::new();
    if req.as_reader().take(MAX_QUERY_BYTES + 1).read_to_string(&mut query).is_err() {
        return reply("request body is not valid UTF-8".to_string(), 400);
    }
    if query.len() as u64 > MAX_QUERY_BYTES { return reply("query too large".to_string(), 413); }
    let interrupt = timeout.map(Interrupt::with_timeout);
    let budget = memory.map(MemoryBudget::spilling);
    let opts = Options { entailment, interrupt: interrupt.as_ref(), memory_budget: budget.as_ref(), ..opts };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| triplelight::exec::plan_trace::explain(snap, &query, &opts))) {
        Ok(Ok(report)) => reply(report, 200),
        Ok(Err(e)) => reply(e.to_string(), error_code(e.kind)),
        Err(_) => reply("query explanation failed".to_string(), 500),
    }
}

/// One SPARQL query against a snapshot, as an HTTP response. Errors: 400 (bad query), 501 (unsupported), 500 (storage);
/// never an empty result in place of a failure.
fn answer<'a, S: triplelight::storage::Snapshot>(snap: &'a S, q: &str, opts: &Options<'_>, fmt: Format) -> Reply<'a> {
    let before = triplelight::storage::disk_index::cache_trace_enabled().then(triplelight::storage::disk_index::cache_trace_snapshot);
    let phases = triplelight::exec::phase::enabled().then(|| { triplelight::exec::phase::take(); triplelight::exec::phase::take_ranges(); Instant::now() });
    let exec_done = std::cell::Cell::new(None::<Instant>);
    let shape = std::cell::Cell::new((0usize, 0usize));
    let derived = triplelight::storage::derived_trace::enabled().then(|| { triplelight::storage::derived_trace::take(); Instant::now() });
    let response = (|| { match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| query_with(snap, q, opts))) {
        Ok(Ok(r)) => {
            exec_done.set(Some(Instant::now()));
            if let QueryResult::Ids(sol) = &r { shape.set((sol.rows, sol.vars.len())) }
            // SPARQL JSON over id rows streams, unless a trace is on: the traces time the whole output, which a stream
            // spreads over the socket writes.
            let streaming = before.is_none() && phases.is_none() && derived.is_none();
            let (ct, b) = match r {
                QueryResult::Ids(sol) if streaming => {
                    // The head and the first window are produced before the status line, so a dictionary failure there
                    // still answers 500. A failure after that ends the body early (see `JsonBody`).
                    let mut stream = IdsJsonStream::new(sol, snap.has_rdf12_terms());
                    let mut first = String::new();
                    for _ in 0..2 {
                        match stream.next_piece(snap) {
                            Ok(Some(piece)) => first.push_str(&piece),
                            Ok(None) => break,
                            Err(e) => return Reply::Whole(tiny_http::Response::from_string(format!("storage error: {e}")).with_status_code(500)),
                        }
                    }
                    let body: Box<dyn Read + 'a> = Box::new(JsonBody { stream, snap, buf: first.into_bytes(), pos: 0 });
                    let header = tiny_http::Header::from_bytes("Content-Type", "application/sparql-results+json").unwrap();
                    return Reply::Stream(tiny_http::Response::new(tiny_http::StatusCode(200), vec![header], body, None, None));
                }
                QueryResult::Ids(sol) => match serialize_ids_v(&sol, snap, snap.has_rdf12_terms()) {
                    Ok(b) => ("application/sparql-results+json", b),
                    Err(e) => return Reply::Whole(tiny_http::Response::from_string(format!("storage error: {e}")).with_status_code(500)),
                },
                r => match fmt {
                    Format::Tsv => serialize_tsv(&r).unwrap_or_else(|| serialize(&r)),
                    Format::Csv => serialize_csv(&r).unwrap_or_else(|| serialize(&r)),
                    Format::Json => serialize(&r),
                },
            };
            Reply::Whole(tiny_http::Response::from_string(b).with_header(tiny_http::Header::from_bytes("Content-Type", ct).unwrap()))
        }
        Ok(Err(e)) => Reply::Whole(tiny_http::Response::from_string(e.to_string()).with_status_code(error_code(e.kind))),
        Err(e) => {
            let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
            let code = if msg.contains("not supported") { 501 } else { 500 };
            Reply::Whole(tiny_http::Response::from_string(msg).with_status_code(code))
        }
    } })();
    if let Some(before) = before {
        let after = triplelight::storage::disk_index::cache_trace_snapshot();
        for (i,name) in ["spo","sop","pso","pos","osp","ops","dictionary.term","dictionary.id","quads"].iter().enumerate() {
            let d: Vec<u64> = (0..5).map(|j|after[i][j].saturating_sub(before[i][j])).collect();
            eprintln!("block_cache {name} hits={} misses={} pread_calls={} pread_bytes={} evictions={}",d[0],d[1],d[2],d[3],d[4]);
        }
    }
    if let Some(t0) = phases {
        let end = Instant::now();
        let [parse, bgp, group, count, sort, filter, extend, join] = triplelight::exec::phase::take();
        let ranges = triplelight::exec::phase::take_ranges();
        let [index_read, dict_read] = triplelight::storage::disk_index::take_read_share();
        let spilled = triplelight::exec::phase::take_spilled();
        let exec_us = exec_done.get().map_or(0, |e| (e - t0).as_micros());
        let output_us = exec_done.get().map_or(0, |e| (end - e).as_micros());
        let (rows, cols) = shape.get();
        let head: String = q.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect();
        eprintln!("phase_trace total_us={} parse_us={parse} bgp_us={bgp} group_us={group} count_us={count} sort_us={sort} index_read_us={index_read} dict_read_us={dict_read} filter_us={filter} extend_us={extend} join_us={join} spill_bytes={spilled} exec_us={exec_us} output_us={output_us} rows={rows} cols={cols} range_pushdown={ranges} query={head:?}", (end - t0).as_micros());
    }
    if let Some(t) = derived {
        let head: String = q.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect();
        eprintln!("derived_trace ms={:.3} {} query={head:?}", t.elapsed().as_secs_f64() * 1e3, triplelight::storage::derived_trace::take());
    }
    response
}

/// `triplelight serve-store <dir> [port] [--init data.nt]`: persistent store. Queries at /sparql, updates at /update
/// (SPARQL 1.1 protocol: `update=` form field or `application/sparql-update` body). An update is acknowledged only
/// after its WAL record is fsynced.
fn serve_store(args: &[String], auth: &UpdateAuth) {
    let init = args.iter().position(|a| a == "--init").map(|i| std::path::PathBuf::from(&args[i + 1]));
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let ablations = ablations(args);
    let timeout = query_timeout(args);
    let memory = query_memory(args);
    let t = Instant::now();
    let store = if args.iter().any(|a| a == "--disk") {
        triplelight::store::Store::open_with(&args[2], init.as_deref(), Box::new(triplelight::storage::DiskBase { workers }))
    } else {
        triplelight::store::Store::open(&args[2], init.as_deref(), workers)
    }.expect("open store");
    eprintln!("opened {} in {:.2}s ({} pending changes replayed)", args[2], t.elapsed().as_secs_f64(), store.snapshot().delta_len());
    // Statistics describe the base; they plan while the pending changes stay small (`statistics_cover`, #111), and
    // compaction rebuilds them.
    let stats: std::sync::RwLock<Option<(usize, std::sync::Arc<Stats>)>> = std::sync::RwLock::new(None);
    let refresh = |stats: &std::sync::RwLock<Option<(usize, std::sync::Arc<Stats>)>>, snap: &triplelight::store::Overlay| {
        if args.iter().any(|a| a == "--no-stats") { return }
        let st = Stats::from_scans(|| snap.scan_base_for_stats()).expect("statistics");
        *stats.write().unwrap() = Some((snap.base_id(), std::sync::Arc::new(st)));
    };
    refresh(&stats, &store.snapshot());
    let port = args.get(3).filter(|a| !a.starts_with("--")).map(String::as_str).unwrap_or("7878");
    let server = listen(port);
    eprintln!("serving SPARQL at http://127.0.0.1:{port}/sparql and /update");
    let (server, store, stats, refresh) = (&server, &store, &stats, &refresh);
    std::thread::scope(|scope| for _ in 0..workers { scope.spawn(move || {
        for mut req in server.incoming_requests() {
            let url = req.url().to_string();
            let path = url.split('?').next().unwrap_or("");
            if path == "/explain" {
                let snap = store.snapshot();
                let st = stats.read().unwrap().clone();
                let st = st.filter(|(base, _)| snap.delta_len() == 0 && *base == snap.base_id());
                let opts = Options { stats: st.as_ref().map(|(_, s)| &**s), ..ablations };
                let resp = explain(&mut req, &*snap, opts, timeout, memory);
                let _ = req.respond(resp);
                continue;
            }
            // Authorize before reading the body, so an unauthorized client cannot make the server buffer it.
            if path == "/update" && let Err(denied) = auth.check(&req) {
                early_rejection(path, denied.status_code().0);
                let _ = req.respond(denied);
                continue
            }
            let limit = if path == "/update" { u64::MAX } else { MAX_QUERY_BYTES };
            // A body declared too large is refused before any of it is read.
            if declared_length(&req).is_some_and(|n| n > limit) {
                early_rejection(path, 413);
                let _ = req.respond(tiny_http::Response::from_string("query too large").with_status_code(413));
                continue
            }
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
                let entailment = request_entailment(&url, (!ctype.starts_with("application/sparql-query")).then_some(body.as_str()), ablations.entailment);
                let q = if ctype.starts_with("application/sparql-query") { Some(body) } else {
                    url.split_once('?').and_then(|(_, qs)| form_param(qs, "query")).or_else(|| form_param(&body, "query"))
                };
                match (q, entailment) {
                    (None, _) => tiny_http::Response::from_string("missing query").with_status_code(400),
                    (_, Err(e)) => tiny_http::Response::from_string(e).with_status_code(400),
                    (Some(q), Ok(entailment)) => {
                        let snap = store.snapshot();
                        let st = stats.read().unwrap().clone();
                        let st = st.filter(|(base, st)| triplelight::embedded::statistics_cover(*base, st, &snap));
                        let fmt = accept_of(&req);
                        let interrupt = timeout.map(Interrupt::with_timeout);
                        let budget = memory.map(MemoryBudget::spilling);
                        let opts = Options { stats: st.as_ref().map(|(_, s)| &**s), raw_ids: fmt == Format::Json, entailment, interrupt: interrupt.as_ref(), memory_budget: budget.as_ref(), ..ablations };
                        match answer(&*snap, &q, &opts, fmt) { Reply::Whole(r) => r, Reply::Stream(r) => { let _ = req.respond(r); continue } }
                    }
                }
            };
            let _ = req.respond(resp);
        }
    }); });
}

/// Requests answered before their body was read (#123): an unauthorized update, or a declared body over the limit.
/// Each is logged with the running count, so a CI log shows whether that path ran when a client saw a reset.
fn early_rejection(path: &str, status: u16) {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    eprintln!("early rejection {n}: {status} {path}");
}

/// The declared `Content-Length`, if any.
fn declared_length(req: &tiny_http::Request) -> Option<u64> {
    req.headers().iter().find(|h| h.field.equiv("Content-Length")).and_then(|h| h.value.as_str().trim().parse().ok())
}

/// Idle timeouts on every connection (#123). tiny-http reads off an unread request body (one we rejected before
/// reading it) inside the worker that answered, with no cap; the vendored tiny-http (`vendor/tiny_http`) sets these
/// timeouts on each accepted connection, so a read that waits longer than the read timeout for a client's next bytes,
/// or a write that waits longer than the write timeout for a client to take more of an answer, fails and closes the
/// connection, which releases its worker. They bound each wait, not the whole transfer: a slow but steady upload or
/// download keeps going. Defaults: 10 s and 60 s; `TRIPLELIGHT_READ_IDLE_MS` and `TRIPLELIGHT_WRITE_IDLE_MS`
/// override them.
fn idle_timeouts() -> (std::time::Duration, std::time::Duration) {
    let ms = |name: &str, default: u64| std::time::Duration::from_millis(
        std::env::var(name).ok().and_then(|v| v.parse().ok()).filter(|&v: &u64| v > 0).unwrap_or(default));
    (ms("TRIPLELIGHT_READ_IDLE_MS", 10_000), ms("TRIPLELIGHT_WRITE_IDLE_MS", 60_000))
}

/// The server on `port`, its connections under `idle_timeouts`.
fn listen(port: &str) -> tiny_http::Server {
    let listener = std::net::TcpListener::bind(format!("{}:{port}", bind_host())).expect("bind");
    let (read, write) = idle_timeouts();
    tiny_http::Server::from_listener_with_timeouts(listener, None, Some(read), Some(write)).expect("serve")
}

/// Listen address host: 127.0.0.1 unless TRIPLELIGHT_HOST is set, e.g. 0.0.0.0 in a container.
fn bind_host() -> String {
    std::env::var("TRIPLELIGHT_HOST").unwrap_or_else(|_| "127.0.0.1".into())
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

/// Benchmark ablations, all off by default and answer-preserving:
/// `--ablate=wcoj,count,fast-group,range,charpairs,sortprobe,csrpaths` disables the leapfrog triejoin, the exact
/// index-count aggregates, the ID hash-aggregation fast path, the range-filter pushdown, the planner's
/// characteristic-pair join estimates, sort-then-probe and the CSR adjacency for single-predicate path closures. (`--no-stats` turns
/// off the cost-based planner.) Unknown names are refused so a typo cannot silently measure the default.
fn ablations(args: &[String]) -> Options<'static> {
    // Not an ablation: the default entailment regime of requests that name none (`request_entailment`).
    let mut opts = Options { entailment: crate::entailment_arg(args), ..Options::default() };
    for name in args.iter().filter_map(|a| a.strip_prefix("--ablate=")).flat_map(|v| v.split(',')).filter(|n| !n.is_empty()) {
        match name {
            "wcoj" => opts.wcoj = false,
            "count" => opts.count_pushdown = false,
            "fast-group" => opts.general_group = true,
            "range" => opts.range_pushdown = false,
            "charpairs" => opts.char_pairs = false,
            "sortprobe" => opts.sorted_probe = false,
            "csrpaths" => opts.csr_paths = false,
            other => { eprintln!("unknown --ablate name {other:?} (expected wcoj, count, fast-group, range, charpairs, sortprobe, csrpaths)"); std::process::exit(2) }
        }
    }
    if args.iter().any(|a| a.starts_with("--ablate=")) {
        eprintln!("ablations: wcoj={} count_pushdown={} fast_group={} range_pushdown={} char_pairs={} sorted_probe={} csr_paths={}", opts.wcoj, opts.count_pushdown, !opts.general_group, opts.range_pushdown, opts.char_pairs, opts.sorted_probe, opts.csr_paths);
    }
    opts
}

/// `triplelight serve <data> [port]`: load a file into memory and answer queries (no updates).
fn serve_memory(args: &[String]) {
            let snap = load(&args[2], args);
            let port = args.get(3).map(String::as_str).unwrap_or("7878");
            // Optimizer statistics (on by default, --no-stats disables) are built on a background thread so the
            // server answers immediately: until they are ready, joins use the greedy order. Join and leapfrog orders
            // then come from the cost model, which a work gate skips for small queries (round-2 benchmarks).
            let stats: std::sync::RwLock<Option<std::sync::Arc<Stats>>> = std::sync::RwLock::new(None);
            let want_stats = !args.iter().any(|a| a == "--no-stats");
            eprintln!("optimizer statistics: {}", if want_stats { "building in background" } else { "off" });
            let ablations = ablations(args);
            let timeout = query_timeout(args);
            let memory = query_memory(args);
            let server = listen(port);
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
                if url.split('?').next() == Some("/explain") {
                    let st = stats.read().unwrap().clone();
                    let opts = Options { stats: st.as_deref(), ..ablations };
                    let resp = explain(&mut req, snap, opts, timeout, memory);
                    let _ = req.respond(resp);
                    continue;
                }
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                let is_query_body = req.headers().iter().any(|h| h.field.equiv("Content-Type") && h.value.as_str().starts_with("application/sparql-query"));
                let entailment = request_entailment(&url, (!is_query_body).then_some(body.as_str()), ablations.entailment);
                let q = if is_query_body { Some(body) } else {
                    url.split_once('?').and_then(|(_, qs)| form_param(qs, "query")).or_else(|| form_param(&body, "query"))
                };
                let st = stats.read().unwrap().clone();
                let fmt = accept_of(&req);
                let resp = match (q, entailment) {
                    (None, _) => tiny_http::Response::from_string("missing query").with_status_code(400),
                    (_, Err(e)) => tiny_http::Response::from_string(e).with_status_code(400),
                    (Some(q), Ok(entailment)) => {
                        let interrupt = timeout.map(Interrupt::with_timeout);
                        let budget = memory.map(MemoryBudget::spilling);
                        let opts = Options { stats: st.as_deref(), raw_ids: fmt == Format::Json, entailment, interrupt: interrupt.as_ref(), memory_budget: budget.as_ref(), ..ablations };
                        match answer(snap, &q, &opts, fmt) { Reply::Whole(r) => r, Reply::Stream(r) => { let _ = req.respond(r); continue } }
                    }
                };
                let _ = req.respond(resp);
            }
            }); } });
}

#[cfg(test)]
mod accept_tests {
    use super::*;

    #[test]
    fn entailment_comes_from_the_url_then_the_form_then_the_server_default() {
        let rdfs = Entailment::Rdfs;
        assert_eq!(request_entailment("/sparql", None, Entailment::Simple), Ok(Entailment::Simple));
        assert_eq!(request_entailment("/sparql", None, rdfs), Ok(rdfs));
        assert_eq!(request_entailment("/sparql?entailment=rdfs", None, Entailment::Simple), Ok(rdfs));
        assert_eq!(request_entailment("/sparql?entailment=simple", Some("entailment=rdfs"), rdfs), Ok(Entailment::Simple));
        assert_eq!(request_entailment("/sparql", Some("query=ASK%7B%7D&entailment=rdfs"), Entailment::Simple), Ok(rdfs));
        // A typo is refused, never silently answered under the default.
        assert!(request_entailment("/sparql?entailment=owl", None, Entailment::Simple).is_err());
    }

    #[test]
    fn q_zero_is_not_acceptable() {
        assert!(format_from_accept("text/csv;q=0, application/sparql-results+json") == Format::Json);
    }
    #[test]
    fn first_listed_wins_a_tie_and_higher_q_wins_otherwise() {
        assert!(format_from_accept("text/csv, text/tab-separated-values") == Format::Csv);
        assert!(format_from_accept("text/tab-separated-values, text/csv") == Format::Tsv);
        assert!(format_from_accept("text/csv;q=0.5, text/tab-separated-values;q=0.9") == Format::Tsv);
    }
    #[test]
    fn whole_media_types_only_and_json_is_the_default() {
        assert!(format_from_accept("") == Format::Json);
        assert!(format_from_accept("*/*") == Format::Json);
        assert!(format_from_accept("text/csv-schema") == Format::Json);
        assert!(format_from_accept("application/sparql-results+json;q=0.2, text/csv;q=0.8") == Format::Csv);
        assert!(format_from_accept("TEXT/CSV; charset=utf-8") == Format::Csv);
    }
}
