#!/usr/bin/env python3
"""#83 cache-budget sweep (registered in #sota-rdf-db:#83 before any run).

  python3 -I sweep.py --bin BIN --store DIR --suite DIR --label NAME --budgets 0,512,1024,2048,10240 --out DIR [--rounds 3]

Budget 0 means "no --cache-mb" (the default, 1/4 of physical memory); the list must start and end with 0 (the
end run is the same-budget drift control). For each budget: one `BIN serve STORE PORT --disk
[--cache-mb N]` process under `/usr/bin/time -l`; every .rq in SUITE runs ROUNDS times in order over HTTP with the whole
response read. Round 1 is the cold block-cache fill; warm = median of the later rounds. Answers are hashed (projected
variables in order; rows in result order with ORDER BY, else sorted) and must match budget 0's. The server is then
stopped and peak RSS / peak physical footprint are read from time's report. Writes OUT/LABEL.json and prints a table.
Take the machine lock before running; this script does not.
"""
import argparse, hashlib, json, math, os, signal, statistics, subprocess, sys, time, urllib.parse, urllib.request


def answer_key(body, ordered):
    doc = json.loads(body)
    if "boolean" in doc:
        return json.dumps({"boolean": doc["boolean"]})
    head = doc["head"]["vars"]
    rows = [json.dumps([b.get(v) for v in head], sort_keys=True) for b in doc["results"]["bindings"]]
    if not ordered:
        rows.sort()
    return hashlib.sha256((json.dumps(head) + "\n" + "\n".join(rows)).encode()).hexdigest() + f":{len(rows)}"


def query(port, text):
    data = urllib.parse.urlencode({"query": text}).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{port}/sparql", data=data,
                                 headers={"Accept": "application/sparql-results+json"})
    t = time.perf_counter()
    with urllib.request.urlopen(req, timeout=3600) as r:
        body = r.read()
    return (time.perf_counter() - t) * 1000.0, body


def wait_ready(port, proc, limit=1800):
    deadline = time.time() + limit
    while time.time() < deadline:
        if proc.poll() is not None:
            raise RuntimeError("server exited during startup")
        try:
            query(port, "ASK { }")
            return
        except Exception:
            time.sleep(0.5)
    raise RuntimeError("server not ready")


def time_report(stderr_text):
    out = {}
    for line in stderr_text.splitlines():
        parts = line.strip().split(None, 1)
        if len(parts) == 2 and parts[0].isdigit():
            if parts[1] == "maximum resident set size":
                out["max_rss_bytes"] = int(parts[0])
            elif parts[1] == "peak memory footprint":
                out["peak_footprint_bytes"] = int(parts[0])
    return out


def run_budget(a, budget, queries):
    cmd = ["/usr/bin/time", "-l", a.bin, "serve", a.store, str(a.port), "--disk"]
    if budget:
        cmd += ["--cache-mb", str(budget)]
    err_path = os.path.join(a.out, f"{a.label}-{budget}.server.log")
    err = open(err_path, "w")
    t0 = time.time()
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=err)
    try:
        wait_ready(a.port, proc)
        open_s = time.time() - t0
        # Process memory before any query: wait for background statistics to settle (RSS stable for 3 samples).
        server = subprocess.run(["pgrep", "-P", str(proc.pid)], capture_output=True, text=True).stdout.split()[0]
        rss = lambda: int(subprocess.run(["ps", "-o", "rss=", "-p", server], capture_output=True, text=True).stdout.strip() or 0) * 1024
        samples = [rss()]
        while len(samples) < 3 or len(set(samples[-3:])) > 1 and len(samples) < 120:
            time.sleep(1.0)
            samples.append(rss())
        idle_rss = samples[-1]
        times = {q: [] for q in queries}
        keys = {q: set() for q in queries}
        for _ in range(a.rounds):
            for q, text in queries.items():
                ms, body = query(a.port, text)
                times[q].append(ms)
                keys[q].add(answer_key(body, "ORDER BY" in text.upper()))
    finally:
        child = subprocess.run(["pgrep", "-P", str(proc.pid)], capture_output=True, text=True).stdout.split()
        for pid in child:
            os.kill(int(pid), signal.SIGTERM)
        proc.wait(timeout=600)
        err.close()
    report = time_report(open(err_path).read())
    return {"budget_mb": budget, "open_s": round(open_s, 2), "idle_rss_bytes": idle_rss, "times_ms": times,
            "keys": {q: sorted(k) for q, k in keys.items()}, **report}


def geomean(xs):
    return math.exp(sum(math.log(max(x, 1e-6)) for x in xs) / len(xs))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--store", required=True)
    ap.add_argument("--suite", required=True)
    ap.add_argument("--label", required=True)
    ap.add_argument("--budgets", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--rounds", type=int, default=3)
    ap.add_argument("--port", type=int, default=7891)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    queries = {f: open(os.path.join(a.suite, f)).read() for f in sorted(os.listdir(a.suite)) if f.endswith(".rq")}
    # The list starts with 0 (the baseline) and ends with 0 again: the repeat is the same-budget drift control.
    budgets = [int(b) for b in a.budgets.split(",")]
    assert budgets[0] == 0 and budgets[-1] == 0 and len(budgets) > 2
    runs = []
    for b in budgets:
        runs.append(run_budget(a, b, queries))
        print(f"budget {b}: done, rss {runs[-1].get('max_rss_bytes', 0) / 2**30:.2f} GiB", flush=True)
    base = runs[0]
    failures = []
    rows = []
    for i, b in enumerate(budgets):
        r = runs[i]
        warm = {q: statistics.median(t[1:]) for q, t in r["times_ms"].items()}
        cold = {q: t[0] for q, t in r["times_ms"].items()}
        bw = {q: statistics.median(t[1:]) for q, t in base["times_ms"].items()}
        bc = {q: t[0] for q, t in base["times_ms"].items()}
        for q in queries:
            if len(r["keys"][q]) != 1 or r["keys"][q] != base["keys"][q]:
                failures.append(f"budget {b} {q}: answers differ")
        wr = [warm[q] / bw[q] for q in queries]
        cr = [cold[q] / bc[q] for q in queries]
        worst = max(queries, key=lambda q: warm[q] / bw[q])
        rows.append({"budget_mb": b, "control": i == len(budgets) - 1, "warm_geomean_ratio": round(geomean(wr), 4), "cold_geomean_ratio": round(geomean(cr), 4),
                     "worst_warm": [worst, round(warm[worst] / bw[worst], 3)], "warm_total_ms": round(sum(warm.values()), 1),
                     "max_rss_gib": round(r.get("max_rss_bytes", 0) / 2**30, 3),
                     "peak_footprint_gib": round(r.get("peak_footprint_bytes", 0) / 2**30, 3),
                     "idle_rss_gib": round(r["idle_rss_bytes"] / 2**30, 3), "open_s": r["open_s"]})
    json.dump({"label": a.label, "store": a.store, "rounds": a.rounds, "summary": rows, "runs": runs, "failures": failures},
              open(os.path.join(a.out, f"{a.label}.json"), "w"), indent=1)
    print(f"{a.label}: budget_mb warm_gm cold_gm worst_warm warm_total_ms idle_rss_gib peak_rss_gib footprint_gib open_s")
    for r in rows:
        print(f"  {r['budget_mb']:>6}{' (control)' if r['control'] else ''} {r['warm_geomean_ratio']:.3f} {r['cold_geomean_ratio']:.3f} "
              f"{r['worst_warm'][0]}={r['worst_warm'][1]} {r['warm_total_ms']} {r['idle_rss_gib']} {r['max_rss_gib']} "
              f"{r['peak_footprint_gib']} {r['open_s']}")
    print("FAILURES:\n" + "\n".join(failures) if failures else "answers match at every budget")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
