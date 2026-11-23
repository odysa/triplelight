#!/usr/bin/env python3
"""S1 per-query memory and I/O (owner Alex): joins a driver result (warm or cold JSON, runs with t0/t1) with the
s1-sample.sh trace of the same session.
  python3 -I s1-report.py <system>.warm.json <system>.s1.tsv > <system>.s1.json
Per query: peak anon and file during its timed runs, device bytes read and major faults (differences across the runs),
and whether an OOM kill happened in them. A run whose server was OOM-killed is reported as a failure, never as a time."""
import json, sys
res = json.load(open(sys.argv[1]))
rows = []   # a `restart` row starts a new cgroup whose counters begin at zero: it is kept as a segment boundary
for line in open(sys.argv[2]):
    if line.startswith("#") or not line.strip(): continue
    f = line.rstrip("\n").split("\t")
    if len(f) > 1 and f[1] == "restart": rows.append([float(f[0]), None, f[2] if len(f) > 2 else ""]); continue
    try: rows.append([float(f[0])] + [int(x) if x else 0 for x in f[1:]])
    except ValueError: continue
def increase(w, i):
    """Total increase of counter column i over w, summed within segments (restart rows split them)."""
    total, prev = 0, None
    for r in w:
        if r[1] is None: prev = None; continue
        if prev is not None and r[i] >= prev: total += r[i] - prev
        prev = r[i]
    return total
def window(t0, t1):
    """Samples inside [t0, t1] exactly. A run shorter than the 0.5 s sampling interval may contain none; then the first
    sample after t1 stands in (it can include the next query's start), and the query is marked `approx`."""
    w = [r for r in rows if t0 <= r[0] <= t1]
    if any(r[1] is not None for r in w): return w, False
    after = [r for r in rows if r[0] > t1 and r[1] is not None][:1]
    return after, True
out = {"system": res.get("system"), "mode": res.get("mode"), "samples": len(rows), "queries": {}}
for name, e in res["queries"].items():
    runs = [r for r in e.get("runs", []) if "t0" in r]
    if not runs or not rows: out["queries"][name] = {"note": "no samples"}; continue
    w, approx = window(runs[0]["t0"], runs[-1]["t1"])
    data = [r for r in w if r[1] is not None]
    if not data: out["queries"][name] = {"note": "no samples in the query's window"}; continue
    out["queries"][name] = {
        "class": e.get("class"), "median_s": e.get("median_s"),
        "peak_anon_bytes": max(r[3] for r in data), "peak_file_bytes": max(r[4] for r in data),
        "major_faults": increase(w, 5), "device_read_bytes": increase(w, 6),
        "approx": approx,
        "oom_kill": increase(w, 7) > 0 or any(r[1] is None and r[2] == "oom-kill" for r in w), "restarts_in_window": sum(1 for r in w if r[1] is None),
    }
    if out["queries"][name]["oom_kill"]: out["queries"][name]["class"] = "oom-killed"
json.dump(out, sys.stdout, indent=1)
