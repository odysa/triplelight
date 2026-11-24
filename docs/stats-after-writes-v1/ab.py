# ABBA for #111: planner statistics through a small delta. Each block clones a LUBM store directory, starts
# `serve-store` on it (statistics are computed before it binds), applies ONE insert, then times every query 10 times
# after 2 warm-ups. On main the insert makes the planner drop its statistics (greedy order); on the branch it keeps them.
import subprocess, hashlib, statistics as st, collections, os, time, urllib.parse, http.client, shutil, glob
STORE = os.environ["STORE_DIR"]; QD = os.environ["QUERIES"]; WORK = os.environ.get("WORK", "/tmp/stats-delta-ab")
def order(f):
    n = os.path.basename(f)[:-3]
    return (0, int(n[1:]), n) if n[1:].isdigit() else (1, 0, n)
Q = {os.path.basename(f)[:-3]: open(f).read() for f in sorted(glob.glob(f"{QD}/*.rq"), key=order)}
ARMS = os.environ.get("ARMS", "main,cand").split(",")
d = collections.defaultdict(lambda: collections.defaultdict(list)); h = collections.defaultdict(set)
def post(field, text):
    body = urllib.parse.urlencode({field: text}); path = "/update" if field == "update" else "/sparql"
    t = time.perf_counter()
    c = http.client.HTTPConnection("127.0.0.1", 7979, timeout=600)
    c.request("POST", path, body, {"Content-Type": "application/x-www-form-urlencoded", "Accept": "application/sparql-results+json"})
    r = c.getresponse(); out = r.read(); c.close()
    assert r.status == 200, (r.status, out[:300])
    return (time.perf_counter() - t) * 1000, hashlib.sha256(out).hexdigest()[:12]
for arm in ARMS + ARMS[::-1]:
    shutil.rmtree(WORK, ignore_errors=True)
    subprocess.run(["cp", "-cR", STORE, WORK], check=True)  # APFS clone: cheap and identical per block
    log = open(f"server-{arm}.log", "w")
    srv = subprocess.Popen([os.environ["TL_" + arm.upper()], "serve-store", WORK, "7979", "--allow-unauthenticated-updates"], env={**os.environ, "TRIPLELIGHT_HOST": "127.0.0.1"}, stdout=log, stderr=log)
    while subprocess.run(["curl", "-sf", "-o", "/dev/null", "--data-urlencode", "query=ASK{?s ?p ?o}", "http://127.0.0.1:7979/sparql"]).returncode: time.sleep(1)
    try:
        if arm not in os.environ.get("NO_UPDATE", "").split(","):
            post("update", "INSERT DATA { <urn:stats-delta-probe> <urn:p> <urn:o> }")
        for k, q in Q.items():
            for i in range(12):
                ms, sh = post("query", q); h[k].add(sh)
                if i >= 2: d[k][arm].append(ms)
            print(arm, k, f"{st.median(d[k][arm][-10:]):.2f}", sh, flush=True)
    finally:
        srv.terminate(); srv.wait()
for k in Q:
    med = {a: st.median(d[k][a]) for a in ARMS}
    print(f"SUMMARY {k:12} " + "  ".join(f"{a} {med[a]:9.2f} ms" for a in ARMS) + "  " + "  ".join(f"{a}/{ARMS[0]} {med[a]/med[ARMS[0]]:.3f}" for a in ARMS[1:]) + f"  {'same' if len(h[k]) == 1 else 'DIFFER ' + str(h[k])}")
