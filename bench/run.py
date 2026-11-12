#!/usr/bin/env python3
"""Run a query set against SPARQL endpoints; report median warm latency and check answers match.
usage: run.py <query_dir> name=url [name=url ...]   (first endpoint is the reference for answer equality)"""
import json, statistics, sys, time, urllib.parse, urllib.request, hashlib, pathlib

RUNS = 5

def ask(url, q):
    data = urllib.parse.urlencode({"query": q}).encode()
    req = urllib.request.Request(url, data=data, headers={"Accept": "application/sparql-results+json",
                                                          "Content-Type": "application/x-www-form-urlencoded"})
    t = time.perf_counter()
    with urllib.request.urlopen(req, timeout=600) as r:
        body = r.read()
    return time.perf_counter() - t, json.loads(body)

def canon(res):
    if "boolean" in res: return 1, str(res["boolean"])
    vs = res["head"]["vars"]
    def val(b):
        if b is None: return "-"
        v = b["value"] if b["type"] != "bnode" else "_"   # blank labels differ across systems
        return f'{b["type"]}|{v}|{b.get("datatype","")}|{b.get("xml:lang","")}'
    rows = sorted("\t".join(val(row.get(v)) for v in vs) for row in res["results"]["bindings"])
    return len(rows), hashlib.sha1("\n".join(rows).encode()).hexdigest()[:10]

def main():
    qdir = pathlib.Path(sys.argv[1])
    eps = [a.split("=", 1) for a in sys.argv[2:]]
    queries = sorted(qdir.glob("*.rq"), key=lambda p: (p.stem.rstrip("0123456789"), int("".join(c for c in p.stem if c.isdigit()) or 0)))
    out = {}
    print("query  " + "  ".join(f"{n:>22}" for n, _ in eps))
    for qf in queries:
        q = qf.read_text()
        cells, ref = [], None
        for name, url in eps:
            try:
                ask(url, q)  # warm-up
                times, res = [], None
                for _ in range(RUNS):
                    t, res = ask(url, q); times.append(t)
                n, h = canon(res)
                med = statistics.median(times) * 1e3
                ok = "" if ref is None or ref == (n, h) else " MISMATCH"
                ref = ref or (n, h)
                cells.append(f"{med:9.2f}ms n={n:<7}{ok}")
                out.setdefault(qf.stem, {})[name] = {"median_ms": med, "rows": n, "hash": h, "all_ms": [x * 1e3 for x in times]}
            except Exception as e:
                cells.append(f"{'ERR ' + type(e).__name__:>22}")
                out.setdefault(qf.stem, {})[name] = {"error": str(e)[:200]}
        print(f"{qf.stem:<6} " + "  ".join(f"{c:>22}" for c in cells), flush=True)
    json.dump(out, open(f"results_{qdir.name}_{int(time.time())}.json", "w"), indent=1)

main()
