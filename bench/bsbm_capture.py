#!/usr/bin/env python3
"""Logging SPARQL proxy: forwards to an upstream endpoint and records every query the BSBM test driver sends,
so one seeded driver run yields a fixed, instantiated query set (bench/bsbm/*.rq) usable by run.py on any system.
usage: bsbm_capture.py <listen_port> <upstream_url> <out_dir>"""
import http.server, sys, urllib.parse, urllib.request, pathlib, itertools
port, upstream, out = int(sys.argv[1]), sys.argv[2], pathlib.Path(sys.argv[3])
out.mkdir(parents=True, exist_ok=True)
counter = itertools.count(1)
class H(http.server.BaseHTTPRequestHandler):
    def _q(self, body):
        qs = urllib.parse.urlparse(self.path).query
        q = urllib.parse.parse_qs(qs).get("query") or urllib.parse.parse_qs(body).get("query")
        return q[0] if q else None
    def handle_query(self, body=""):
        q = self._q(body)
        if q:
            (out / f"q{next(counter):03d}.rq").write_text(q)
        data = urllib.parse.urlencode({"query": q or ""}).encode()
        req = urllib.request.Request(upstream, data=data, headers={"Accept": self.headers.get("Accept", "application/sparql-results+json")})
        try:
            with urllib.request.urlopen(req, timeout=600) as r:
                b = r.read(); self.send_response(r.status); self.send_header("Content-Type", r.headers.get("Content-Type", "application/sparql-results+json"))
        except urllib.error.HTTPError:
            # Capture mode: keep the driver going past unsupported forms (e.g. DESCRIBE); answers are not used.
            b = b'{"head":{"vars":[]},"results":{"bindings":[]}}'; self.send_response(200); self.send_header("Content-Type", "application/sparql-results+json")
        self.send_header("Content-Length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def do_GET(self): self.handle_query()
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0)); self.handle_query(self.rfile.read(n).decode())
    def log_message(self, *a): pass
http.server.ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
