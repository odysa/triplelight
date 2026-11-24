# Warm-server ABBA for the star count path (#105): blocks main, branch, branch, main; each block starts its own server
# on the WatDiv-100M store, warms every query twice, then times it 20 times (in-process HTTP, to the last byte).
import subprocess, hashlib, statistics as st, collections, os, time, urllib.parse, sys
S = os.environ["WATDIV100_STORE"]; QD = os.environ["QUERIES"]
# Every agg-WatDiv star (the queries whose path changes), plus two non-star controls.
STARS = ["C3", "L3", "L4", "S1", "S2", "S3", "S4", "S5", "S6", "S7"]
NAMES = [f"{a}_{s}" for s in STARS for a in ("all", "distinct")] + ["all_F5", "all_L2"]
Q = {n: open(f"{QD}/{n}.rq").read() for n in NAMES if os.path.exists(f"{QD}/{n}.rq")}
d = collections.defaultdict(lambda: collections.defaultdict(list)); h = collections.defaultdict(set)
import http.client
def ask(q):
    # In-process HTTP, one connection per request: no curl process start in the timed span (about 8 ms on this laptop).
    body = urllib.parse.urlencode({"query": q})
    t = time.perf_counter()
    c = http.client.HTTPConnection("127.0.0.1", 7979)
    c.request("POST", "/sparql", body, {"Content-Type": "application/x-www-form-urlencoded", "Accept": "application/sparql-results+json"})
    r = c.getresponse(); out = r.read(); c.close()
    assert r.status == 200, (r.status, out[:200])
    return (time.perf_counter() - t) * 1000, hashlib.sha256(out).hexdigest()[:12]
for arm in ["main", "cand", "cand", "main"]:
    srv = subprocess.Popen([os.environ["TL_" + arm.upper()], "serve", S, "7979", "--disk", "--cache-mb", "8192"], env={**os.environ, "TRIPLELIGHT_HOST": "127.0.0.1"}, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    while subprocess.run(["curl", "-sf", "-o", "/dev/null", "--data-urlencode", "query=ASK{?s ?p ?o}", "http://127.0.0.1:7979/sparql"]).returncode: time.sleep(1)
    try:
        for k, q in Q.items():
            for i in range(22):
                ms, sh = ask(q); h[k].add(sh)
                if i >= 2: d[k][arm].append(ms)
            print(arm, k, f"{st.median(d[k][arm][-20:]):.2f}", sh, flush=True)
    finally:
        srv.terminate(); srv.wait()
for k in Q:
    m = st.median(d[k]["main"]); c = st.median(d[k]["cand"])
    print(f"SUMMARY {k:12} main {m:8.2f} ms  branch {c:8.2f} ms  ratio {c/m:.3f}  {'same' if len(h[k]) == 1 else 'DIFFER ' + str(h[k])}")
