#!/usr/bin/env python3
"""Merge per-system result files of one dataset: medians, answer equality vs the first system, load/RSS."""
import json, glob, os, re, statistics, sys
d = sys.argv[1]; systems = sys.argv[2:]
res = {}
for s in systems:
    f = f"results/{d}/{s}.json"
    if os.path.exists(f): res[s] = json.load(open(f))
ref = systems[0]
qs = list(res[ref].keys())
print("q     " + "".join(f"{s:>14}" for s in res) + "     rows  same")
for q in qs:
    h = res[ref][q][ref].get("hash"); same = True; cells = ""
    for s, r in res.items():
        x = r.get(q, {}).get(s, {})
        cells += f"{x['median_ms']:14.2f}" if "median_ms" in x else f"{'-':>14}"
        if "hash" in x: same &= x["hash"] == h
    print(f"{q:<5} {cells} {res[ref][q][ref].get('rows', '-'):>8}  {same}")
for s, r in res.items():
    v = [r[q][s]["median_ms"] for q in qs if "median_ms" in r.get(q, {}).get(s, {})]
    log = open(f"results/{d}/{s}.server.log").read() if os.path.exists(f"results/{d}/{s}.server.log") else ""
    ld = (open(f"results/{d}/{s}.load.log").read().strip() if os.path.exists(f"results/{d}/{s}.load.log") else "")
    loaded = re.findall(r"loaded.*?in ([\d.]+)s", log); rss = re.findall(r"rss_mb (\d+)", log)
    print(f"{s:14} n={len(v):2} geomean={statistics.geometric_mean(v):9.2f}ms total={sum(v):10.1f}ms load={loaded or ld} rss_mb={rss}")
