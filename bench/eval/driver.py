#!/usr/bin/env python3
"""Evaluation driver (docs/eval-plan/PLAN.md, section "Server suites ... and the driver").

  driver.py reference --queries DIR --url URL --out ref.json     answers of the reference system (Jena), untimed
  driver.py run --system NAME --url URL --queries DIR --reference ref.json --out out.json [--mode warm|cold]
  driver.py selftest                                             classifier and streaming checks on fake endpoints

One system at a time. Every response is hashed while it streams (rows are not kept), so a 7.9M-row answer needs bounded
memory. Each query and system gets a record with a class: pass, value-compatible, mismatch, incomplete, timeout, error.
Standard library only."""
import argparse, codecs, hashlib, http.client, json, os, platform, random, re, statistics, subprocess, sys, time
import urllib.parse

XSD = "http://www.w3.org/2001/XMLSchema#"
# Datatypes that some systems report as xsd:integer and others as a narrower integer type (QLever returns xsd:int).
INTEGER_LIKE = {XSD + t for t in ("int", "long", "short", "byte", "nonNegativeInteger", "positiveInteger", "negativeInteger",
                                  "nonPositiveInteger", "unsignedInt", "unsignedLong", "unsignedShort", "unsignedByte")}
CAP = 1048576   # Virtuoso's HTTP endpoint stops here whatever ResultSetMaxRows says
CHUNK = 1 << 20


MIN_READ = 64 << 10      # stream_fold gathers chunks up to this size per read; the self-test sets 0 to exercise awkward chunk boundaries
class Timeout(Exception): pass
class Truncated(Exception): pass


def term(b, normalize):
    """Canonical text of one binding, as in bench/run.py; `normalize` maps narrower integer types to xsd:integer."""
    if b is None: return "-"
    kind = b["type"]
    v = b["value"] if kind != "bnode" else "_"                      # blank labels differ across systems
    kind = "literal" if kind == "typed-literal" else kind           # Virtuoso's SPARQL-JSON 2008 spelling
    dt = b.get("datatype", "")
    if dt == XSD + "string": dt = ""                                # RDF 1.1: a plain literal is an xsd:string
    if normalize and dt in INTEGER_LIKE: dt = XSD + "integer"
    return f'{kind}|{v}|{dt}|{b.get("xml:lang", "").lower()}'


class Fold:
    """Order-independent bag hash (sum of 128-bit digests), an order-sensitive hash, and a row count; two variants,
    exact and with narrower integer types normalized."""
    def __init__(self, vars_):
        self.vars, self.n, self.bag, self.bag_norm = vars_, 0, 0, 0
        self.seq = hashlib.sha1()
    def add(self, row):
        a = "\t".join(term(row.get(v), False) for v in self.vars)
        b = "\t".join(term(row.get(v), True) for v in self.vars)
        self.n += 1
        self.bag = (self.bag + int.from_bytes(hashlib.blake2b(a.encode(), digest_size=16).digest(), "big")) % (1 << 128)
        self.bag_norm = (self.bag_norm + int.from_bytes(hashlib.blake2b(b.encode(), digest_size=16).digest(), "big")) % (1 << 128)
        self.seq.update(a.encode() + b"\n")
    def result(self):
        return {"rows": self.n, "bag": f"{self.bag:032x}", "bag_norm": f"{self.bag_norm:032x}", "seq": self.seq.hexdigest()}


def fold_boolean(value):
    f = Fold([]); f.n = 1
    h = int.from_bytes(hashlib.blake2b(str(bool(value)).encode(), digest_size=16).digest(), "big")
    f.bag = f.bag_norm = h; f.seq.update(str(bool(value)).encode())
    return f.result()


def stream_fold(chunks, deadline):
    """Fold a SPARQL JSON result read from `chunks` (an iterator of bytes) without keeping the rows.
    Raises Truncated if the document ends inside the bindings array, Timeout past `deadline`."""
    dec = json.JSONDecoder()
    text = codecs.getincrementaldecoder("utf-8")()
    buf, pos, ended = "", 0, False

    def more():
        nonlocal buf, pos, ended
        if time.monotonic() > deadline: raise Timeout()
        # gather small chunks into one read of at least 64 KB: `buf += ...` copies the buffer, so tiny chunks (the self-test
        # feeds one binding at a time) made the run slow and fragmented the heap (565 MB peak at 5M rows, flat from 100k rows up)
        parts, size = [], 0
        try:
            while True:
                c = next(chunks); parts.append(c); size += len(c)
                if size >= MIN_READ: break
        except StopIteration:
            ended = True
            if not parts: return False
        if pos > (1 << 20): buf, pos = buf[pos:], 0
        buf += text.decode(b"".join(parts))
        return True

    # head: read until the bindings array (or a boolean) starts
    while True:
        m = re.search(r'"bindings"\s*:\s*\[', buf)
        if m: break
        if '"boolean"' in buf and not re.search(r'"bindings"', buf):
            while more(): pass          # an ASK answer is tiny: read it all
            return fold_boolean(json.loads(buf)["boolean"])
        if not more():
            raise Truncated("document ended before the bindings array")
    head = buf[:m.start()]
    vm = re.search(r'"vars"\s*:\s*(\[[^\]]*\])', head)
    if not vm:
        # results before head: read the whole document and fold it the plain way (small answers only)
        while more(): pass
        doc = json.loads(buf); f = Fold(doc["head"]["vars"])
        for row in doc["results"]["bindings"]: f.add(row)
        return f.result()
    fold = Fold(json.loads(vm.group(1)))
    pos = m.end()
    while True:
        while True:
            while pos < len(buf) and buf[pos] in " \t\r\n,": pos += 1
            if pos < len(buf): break
            if not more(): raise Truncated("document ended inside the bindings array")
        if buf[pos] == "]": return fold.result()
        try:
            obj, end = dec.raw_decode(buf, pos)
        except ValueError:
            if not more(): raise Truncated("document ended inside a binding")
            continue
        fold.add(obj); pos = end


def post(url, query, timeout):
    """POST the query; returns (status, chunk iterator, connection). The iterator yields the body in chunks."""
    u = urllib.parse.urlsplit(url)
    conn = http.client.HTTPConnection(u.hostname, u.port, timeout=timeout)
    body = urllib.parse.urlencode({"query": query})
    conn.request("POST", u.path or "/", body, {"Accept": "application/sparql-results+json",
                                              "Content-Type": "application/x-www-form-urlencoded"})
    resp = conn.getresponse()
    def chunks():
        while True:
            c = resp.read(CHUNK)
            if not c: return
            yield c
    return resp.status, chunks(), conn


def one_run(url, query, timeout):
    """One timed execution to the last byte. Returns (record dict, seconds)."""
    t0 = time.monotonic(); deadline = t0 + timeout
    try:
        status, chunks, conn = post(url, query, timeout)
        try:
            if status != 200:
                body = b"".join(chunks)[:200]
                return {"status": "error", "detail": f"HTTP {status}: {body!r}"}, time.monotonic() - t0
            rec = stream_fold(chunks, deadline)
        finally:
            conn.close()
        rec["status"] = "ok"
        return rec, time.monotonic() - t0
    except (Timeout, TimeoutError):
        return {"status": "timeout"}, time.monotonic() - t0
    except Truncated as e:
        return {"status": "truncated", "detail": str(e)}, time.monotonic() - t0
    except OSError as e:
        if "timed out" in str(e): return {"status": "timeout"}, time.monotonic() - t0
        return {"status": "error", "detail": f"{type(e).__name__}: {e}"[:200]}, time.monotonic() - t0
    except (ValueError, KeyError) as e:
        return {"status": "error", "detail": f"unparsable answer: {e}"[:200]}, time.monotonic() - t0


# QLever's two-order index (PSO and POS only) refuses a query with a variable predicate with this sentence. Only this text is
# classified `unsupported` (an engine that cannot answer the pattern at all); every other error stays `error`.
UNSUPPORTED_QLEVER_TWO_ORDER = "contains a predicate variable, but only the PSO and POS permutations were loaded"


def classify(rec, ref):
    """pass | value-compatible | mismatch | incomplete | timeout | error, for one run record against the reference."""
    s = rec["status"]
    if s == "timeout": return "timeout"
    if s == "error": return "unsupported" if UNSUPPORTED_QLEVER_TWO_ORDER in rec.get("detail", "") else "error"
    if s == "truncated": return "incomplete"
    if rec["rows"] == ref["rows"] and rec["bag"] == ref["bag"]: return "pass"
    if rec["rows"] == ref["rows"] and rec["bag_norm"] == ref["bag_norm"]: return "value-compatible"
    if rec["rows"] < ref["rows"] and rec["rows"] == CAP: return "incomplete"
    return "mismatch"


def load_queries(qdir):
    import pathlib
    files = sorted(pathlib.Path(qdir).glob("*.rq"), key=lambda p: (p.stem.rstrip("0123456789"), int("".join(c for c in p.stem if c.isdigit()) or 0)))
    return [(f.stem, f.read_text()) for f in files]


def git_sha():
    try: return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=os.path.dirname(os.path.abspath(__file__)), text=True).strip()
    except Exception: return None


def versions():
    out = {}
    try:
        for line in open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "VERSIONS")):
            if "=" in line and not line.startswith("#"): k, v = line.strip().split("=", 1); out[k] = v
    except OSError: pass
    return out


def cmd_reference(a):
    ref = {}
    for name, q in load_queries(a.queries):
        rec, secs = one_run(a.url, q, a.timeout)
        if rec["status"] != "ok": sys.exit(f"reference run of {name} failed: {rec}")
        rec["ordered"] = "ORDER BY" in q.upper(); ref[name] = rec
        print(f"{name}: {rec['rows']} rows", flush=True)
    json.dump({"reference_url": a.url, "git": git_sha(), "versions": versions(), "queries": ref}, open(a.out, "w"), indent=1)


def run_hook(hook, url):
    """Run the cold-start hook (restart the server, drop the page cache); then wait until the endpoint answers."""
    r = subprocess.run(hook, shell=True, capture_output=True, text=True)
    ok = r.returncode == 0
    deadline = time.time() + 600
    while time.time() < deadline:
        # constant predicate: QLever's two-order index cannot answer a variable predicate, so ASK { ?s ?p ?o } would never succeed there
        rec, _ = one_run(url, "ASK { ?s <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> ?o }", 30)
        if rec["status"] == "ok": break
        time.sleep(2)
    else: ok = False
    return ok, (r.stdout + r.stderr)[-300:]


def cmd_run(a):
    ref = json.load(open(a.reference))["queries"]
    records = {}
    rng = random.Random(a.seed)
    queries = load_queries(a.queries)
    for name, q in queries:
        if name not in ref: sys.exit(f"{name} has no reference answer")
        r = ref[name]; entry = {"mode": a.mode, "runs": []}
        if a.mode == "cold":
            if not a.cold_hook: sys.exit("--mode cold needs --cold-hook")
            ok, note = run_hook(a.cold_hook, a.url)
            entry["cold_hook_ok"], entry["cold_hook_note"] = ok, note
            runs = 1
        else:
            one_run(a.url, q, a.timeout)       # warm-up, not recorded
            runs = a.runs
        for _ in range(runs):
            rec, secs = one_run(a.url, q, a.timeout)
            cls = classify(rec, r)
            entry["runs"].append({"class": cls, "seconds": round(secs, 4), **{k: rec[k] for k in ("rows", "bag", "seq") if k in rec},
                                  **({"detail": rec["detail"]} if "detail" in rec else {})})
        classes = {x["class"] for x in entry["runs"]}
        entry["class"] = classes.pop() if len(classes) == 1 else "unstable"
        ok_runs = [x["seconds"] for x in entry["runs"] if x["class"] in ("pass", "value-compatible")]
        if ok_runs: entry["median_s"] = round(statistics.median(ok_runs), 4)
        if r.get("ordered") and entry["class"] in ("pass", "value-compatible"):
            entry["order_matches_reference"] = all(x.get("seq") == r["seq"] for x in entry["runs"])
        records[name] = entry
        print(f"{name:<8} {entry['class']:<17} {entry.get('median_s', '-')}", flush=True)
    summary = {}
    for e in records.values(): summary[e["class"]] = summary.get(e["class"], 0) + 1
    json.dump({"system": a.system, "url": a.url, "mode": a.mode, "timeout_s": a.timeout, "runs_per_query": a.runs, "seed": a.seed,
               "git": git_sha(), "versions": versions(), "python": platform.python_version(), "platform": platform.platform(),
               "summary": summary, "queries": records}, open(a.out, "w"), indent=1)
    print("summary:", summary)


# ---------------------------------------------------------------- self-test

def _bindings(n, kind="ok"):
    for i in range(n):
        dt = f',"datatype":"{XSD}{"int" if kind == "int" else "integer"}"'
        v = i + (1 if kind == "wrong" and i == 0 else 0)
        yield f'{{"s":{{"type":"uri","value":"urn:s{i}"}},"n":{{"type":"literal","value":"{v}"{dt}}}}}'


def _body(n, kind="ok"):
    yield '{"head":{"vars":["s","n"]},"results":{"bindings":[\n'
    first = True
    for b in _bindings(n, kind):
        yield ("" if first else ",\n") + b; first = False
    yield "\n]}}"


def serve_fake(behaviors):
    import threading, http.server
    class H(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a): pass
        def do_POST(self):
            self.rfile.read(int(self.headers.get("Content-Length", 0)))
            kind, n = behaviors[self.path]
            if kind == "err": self.send_response(500); self.end_headers(); self.wfile.write(b"boom"); return
            if kind == "unsup": self.send_response(500); self.end_headers(); self.wfile.write(b'{"exception": "The query contains a predicate variable, but only the PSO and POS permutations were loaded. Rerun the server without the option."}'); return
            self.send_response(200); self.send_header("Content-Type", "application/sparql-results+json"); self.end_headers()
            if kind == "slow": time.sleep(3)
            if kind == "ask": self.wfile.write(b'{"head":{},"boolean":true}'); return
            text = "".join(_body(n, kind if kind in ("int", "wrong") else "ok"))
            if kind == "trunc": text = text[: len(text) // 2]
            self.wfile.write(text.encode())
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, srv.server_address[1]


def cmd_selftest(a):
    global CAP
    ref = {"rows": 50, **fold_of(_body(50))}
    ok = True
    cases = [("good", "ok", 50, "pass"), ("typing", "int", 50, "value-compatible"), ("wrong", "wrong", 50, "mismatch"),
             ("short", "ok", 30, "mismatch"), ("trunc", "trunc", 50, "incomplete"), ("err", "err", 0, "error"), ("unsup", "unsup", 0, "unsupported"), ("slow", "slow", 50, "timeout"),
             ("cap", "ok", 40, "incomplete")]
    behaviors = {f"/{name}": (kind, n) for name, kind, n, _ in cases}
    behaviors["/ask"] = ("ask", 0)
    srv, port = serve_fake(behaviors)
    CAP = 40                       # the fake "cap" endpoint returns 40 of the 50 rows
    for name, kind, n, want in cases:
        rec, secs = one_run(f"http://127.0.0.1:{port}/{name}", "SELECT * WHERE {}", 1 if name == "slow" else 30)
        got = classify(rec, ref)
        print(f"{name:<8} want {want:<17} got {got:<17} {'ok' if got == want else 'FAIL'}"); ok &= got == want
    rec, _ = one_run(f"http://127.0.0.1:{port}/ask", "ASK {}", 5)
    good = rec["status"] == "ok" and rec["rows"] == 1 and rec["bag"] == fold_boolean(True)["bag"]
    print("ask      boolean answer", "ok" if good else "FAIL"); ok &= good
    # streaming equals the plain parse on a 100k-row sample, with the document split at awkward chunk sizes
    global MIN_READ
    for rows, size in ((100_000, 1 << 20), (100_000, 4096), (2_000, 7), (2_000, 1)):
        doc = "".join(_body(rows)); plain = Fold(["s", "n"])
        for row in json.loads(doc)["results"]["bindings"]: plain.add(row)
        data = doc.encode()
        MIN_READ = 0                 # every chunk is parsed on its own, so the awkward boundaries are really tested
        try: got = stream_fold(iter([data[i:i + size] for i in range(0, len(data), size)]), time.monotonic() + 600)
        finally: MIN_READ = 64 << 10
        same = got == plain.result(); print(f"stream   {rows} rows, chunks of {size} bytes", "ok" if same else "FAIL"); ok &= same
    print("SELFTEST", "PASSED" if ok else "FAILED"); sys.exit(0 if ok else 1)


def fold_of(parts):
    return stream_fold(iter([p.encode() for p in parts]), time.monotonic() + 600)


def cmd_selftest_big(a):
    """5M rows through the streaming fold in one process; prints the rows folded and the peak RSS in MB."""
    import resource
    rows = a.rows
    r = stream_fold((p.encode() for p in _body(rows)), time.monotonic() + 3600)
    rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    mb = rss / (1 << 20) if sys.platform == "darwin" else rss / 1024
    print(f"rows={r['rows']} peak_rss_mb={mb:.0f}"); sys.exit(0 if r["rows"] == rows else 1)


def main():
    p = argparse.ArgumentParser(); sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("reference"); r.add_argument("--queries", required=True); r.add_argument("--url", required=True)
    r.add_argument("--out", required=True); r.add_argument("--timeout", type=float, default=300); r.set_defaults(f=cmd_reference)
    x = sub.add_parser("run"); x.add_argument("--system", required=True); x.add_argument("--url", required=True)
    x.add_argument("--queries", required=True); x.add_argument("--reference", required=True); x.add_argument("--out", required=True)
    x.add_argument("--mode", choices=["warm", "cold"], default="warm"); x.add_argument("--cold-hook")
    x.add_argument("--runs", type=int, default=5); x.add_argument("--timeout", type=float, default=300); x.add_argument("--seed", type=int, default=0)
    x.set_defaults(f=cmd_run)
    s = sub.add_parser("selftest"); s.set_defaults(f=cmd_selftest)
    b = sub.add_parser("selftest-big"); b.add_argument("--rows", type=int, default=5_000_000); b.set_defaults(f=cmd_selftest_big)
    a = p.parse_args(); a.f(a)


if __name__ == "__main__":
    main()
