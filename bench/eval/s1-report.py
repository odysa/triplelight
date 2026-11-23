#!/usr/bin/env python3
"""S1 per-query memory and I/O (owner Alex): joins a driver result (warm or cold JSON, runs with t0/t1) with the
s1-sample.sh trace of the same session.
  python3 -I s1-report.py <system>.warm.json <system>.s1.tsv > <system>.s1.json
Per query: peak anon and file during its timed runs, device bytes read and major faults (differences across the runs),
and whether an OOM kill happened in them. A run whose server was OOM-killed is reported as a failure, never as a time."""
import json, sys
res = json.load(open(sys.argv[1]))
rows = []
for line in open(sys.argv[2]):
    if line.startswith("#") or not line.strip(): continue
    f = line.rstrip("\n").split("\t")
    try: rows.append([float(f[0])] + [int(x) if x else 0 for x in f[1:]])
    except ValueError: continue
def window(t0, t1):
    w = [r for r in rows if t0 - 0.5 <= r[0] <= t1 + 0.5]
    return w
out = {"system": res.get("system"), "mode": res.get("mode"), "samples": len(rows), "queries": {}}
for name, e in res["queries"].items():
    runs = [r for r in e.get("runs", []) if "t0" in r]
    if not runs or not rows: out["queries"][name] = {"note": "no samples"}; continue
    w = window(runs[0]["t0"], runs[-1]["t1"])
    if not w: out["queries"][name] = {"note": "no samples in the query's window"}; continue
    out["queries"][name] = {
        "class": e.get("class"), "median_s": e.get("median_s"),
        "peak_anon_bytes": max(r[3] for r in w), "peak_file_bytes": max(r[4] for r in w),
        "major_faults": w[-1][5] - w[0][5], "device_read_bytes": w[-1][6] - w[0][6],
        "oom_kill": w[-1][7] - w[0][7] > 0,
    }
    if out["queries"][name]["oom_kill"]: out["queries"][name]["class"] = "oom-killed"
json.dump(out, sys.stdout, indent=1)
