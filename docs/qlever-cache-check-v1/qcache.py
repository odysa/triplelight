#!/usr/bin/env python3
"""QLever result-cache check (#105): same store, same queries, with and without `cmd=clear-cache-complete` before every request.
Time to the last byte, rows. Usage: qcache.py URL OUT.json QUERYFILE..."""
import sys, time, json, urllib.request, urllib.parse
url, out, files = sys.argv[1], sys.argv[2], sys.argv[3:]
def clear():
    r = urllib.request.urlopen(url + "?cmd=clear-cache-complete&access-token=qc", timeout=60); r.read(); return r.status
def run(q):
    t = time.monotonic()
    req = urllib.request.Request(url, data=urllib.parse.urlencode({"query": q}).encode(), headers={"Accept": "application/sparql-results+json"})
    n = 0
    with urllib.request.urlopen(req, timeout=300) as r:
        while True:
            c = r.read(1 << 20)
            if not c: break
            n += len(c)
    return time.monotonic() - t, n
res = {}
try: clear_status = clear()
except Exception as e: clear_status = f"clear failed: {e}"
res["clear_status"] = clear_status
for mode in ("control", "cleared"):
    for f in files:
        q = open(f).read(); name = f.rsplit("/", 1)[-1]
        times = []
        for i in range(4):   # 1 warm-up + 3 timed, as the baseline
            if mode == "cleared": clear()
            times.append(run(q))
        res.setdefault(name, {})[mode] = {"warm_up_s": round(times[0][0], 4), "timed_s": [round(t[0], 4) for t in times[1:]], "bytes": times[-1][1]}
json.dump(res, open(out, "w"), indent=1)
for n, v in res.items():
    if n != "clear_status": print(n, "control", v["control"]["timed_s"], "cleared", v["cleared"]["timed_s"])
