#!/usr/bin/env python3
"""Run a query set against SPARQL endpoints; report median warm latency and check answers match.
usage: run.py <query_dir> name=url [name=url ...]   (first endpoint is the reference for answer equality)"""
import json, statistics, sys, time, urllib.parse, urllib.request, hashlib, pathlib

RUNS = 1

def ask(url, q):
    data = urllib.parse.urlencode({"query": q}).encode()
    req = urllib.request.Request(url, data=data, headers={"Accept": "application/sparql-results+json",
                                                          "Content-Type": "application/x-www-form-urlencoded"})
    t = time.perf_counter()
    with urllib.request.urlopen(req, timeout=600) as r:
        body = r.read()
    return time.perf_counter() - t, json.loads(body)

def canon(res, ordered=False):
    """(rows, bag hash) — plus, for ORDER BY queries, the hash of rows in returned order (sequence check)."""
    if "boolean" in res: return 1, str(res["boolean"]), None
    vs = res["head"]["vars"]
    def val(b):
        if b is None: return "-"
        v = b["value"] if b["type"] != "bnode" else "_"   # blank labels differ across systems
        t = "literal" if b["type"] == "typed-literal" else b["type"]   # Virtuoso uses the SPARQL-JSON 2008 "typed-literal"
        dt = b.get("datatype", "")
        if dt == "http://www.w3.org/2001/XMLSchema#string": dt = ""     # RDF 1.1: plain literal == xsd:string
        return f'{t}|{v}|{dt}|{b.get("xml:lang","").lower()}'
    seq = ["\t".join(val(row.get(v)) for v in vs) for row in res["results"]["bindings"]]
    rows = sorted(seq)
    oh = hashlib.sha1("\n".join(seq).encode()).hexdigest() if ordered else None
    return len(rows), hashlib.sha1("\n".join(rows).encode()).hexdigest(), oh

def main():
    qdir = pathlib.Path(sys.argv[1])
    eps = [a.split("=", 1) for a in sys.argv[2:]]
    queries = sorted(qdir.glob("*.rq"), key=lambda p: (p.stem.rstrip("0123456789"), int("".join(c for c in p.stem if c.isdigit()) or 0)))
    out = {}
    print("query  " + "  ".join(f"{n:>22}" for n, _ in eps))
    for qf in queries:
        q = qf.read_text()
        ordered = "ORDER BY" in q.upper()
        cells, ref, oref = [], None, None
        for name, url in eps:
            try:
                ask(url, q)  # warm-up
                times, answers = [], set()
                for _ in range(RUNS):
                    t, res = ask(url, q); times.append(t); answers.add(canon(res, ordered))
                if len(answers) != 1: raise RuntimeError(f"answers differ across runs: {sorted(answers)}")
                n, h, oh = answers.pop()
                med = statistics.median(times) * 1e3
                ok = "" if ref is None or ref == (n, h) else " MISMATCH"
                # Same bag but different sequence for an ORDER BY query: a wrong sort or a legitimate tie order.
                if not ok and ordered and oref is not None and oh != oref: ok = " ORDER?"
                ref = ref or (n, h); oref = oref or oh
                cells.append(f"{med:9.2f}ms n={n:<7}{ok}")
                out.setdefault(qf.stem, {})[name] = {"median_ms": med, "rows": n, "hash": h, "seq_hash": oh, "all_ms": [x * 1e3 for x in times]}
            except Exception as e:
                cells.append(f"{'ERR ' + type(e).__name__:>22}")
                out.setdefault(qf.stem, {})[name] = {"error": str(e)[:200]}
        print(f"{qf.stem:<6} " + "  ".join(f"{c:>22}" for c in cells), flush=True)
    json.dump(out, open(f"results_{qdir.name}_{int(time.time())}.json", "w"), indent=1)

main()
