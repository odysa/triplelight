#!/usr/bin/env python3
"""S1 per-query block-cache report for TripleLight (owner Alex; docs/eval-plan/PLAN.md "S1", #92/#105).
  python3 -I s1-cache.py <queries-dir> triplelight.warm.json triplelight.server.log > triplelight.s1-cache.json

The server, run with TRIPLELIGHT_TRACE_BLOCK_CACHE=1 and TRIPLELIGHT_TRACE_PHASES=1, prints after every request nine
`block_cache <file> hits= misses= pread_calls= pread_bytes= evictions=` lines and one `phase_trace ... query="<first 80
characters, whitespace collapsed>"` line. The driver issues, per query in file order, one warm-up and then its timed
runs, one at a time. This script takes the last requests of the server log, as many as the driver made, and checks
each against the query it should be (its traced text). Any mismatch, for example a restart after a timeout that
split the log, makes the whole report `"joined": false` rather than a guess.

Per query, over its timed runs: block-cache hits, misses and hit rate, bytes read from the store files, read calls
and evictions, split into index orders and dictionary. No timing claim: the times stay in the driver's JSON."""
import json, pathlib, re, sys

ORDERS = ("spo", "sop", "pso", "pos", "osp", "ops", "quads")
DICTIONARY = ("dictionary.term", "dictionary.id")
LINE = re.compile(r"^block_cache (\S+) hits=(\d+) misses=(\d+) pread_calls=(\d+) pread_bytes=(\d+) evictions=(\d+)$")
QUERY = re.compile(r'query="(.*)"$')


def requests(log):
    """One dict per traced request: {file: [hits, misses, calls, bytes, evictions]} plus its traced query text."""
    out, cur = [], {}
    for line in open(log, errors="replace"):
        line = line.rstrip("\n")
        m = LINE.match(line)
        if m:
            cur[m.group(1)] = [int(x) for x in m.groups()[1:]]
        elif line.startswith("phase_trace "):
            q = QUERY.search(line)
            out.append({"files": cur, "query": json.loads('"' + q.group(1) + '"') if q else None})
            cur = {}
    return out


def head(text):
    return " ".join(text.split())[:80]


def main(qdir, warm, log):
    res = json.load(open(warm))
    texts = {p.stem: p.read_text() for p in pathlib.Path(qdir).glob("*.rq")}
    plan = []   # (query, is_timed) in the order the driver issued them
    for name, entry in res["queries"].items() if "queries" in res else res.items():
        if not isinstance(entry, dict) or "runs" not in entry: continue
        plan.append((name, False))
        plan += [(name, True)] * len(entry["runs"])
    reqs = requests(log)
    report = {"joined": False, "requests_in_log": len(reqs), "requests_expected": len(plan), "queries": {}}
    if len(reqs) < len(plan):
        report["why"] = "fewer traced requests than the driver made (was the server run with both trace variables?)"
        return report
    reqs = reqs[-len(plan):]
    for (name, _), r in zip(plan, reqs):
        if name in texts and r["query"] is not None and r["query"] != head(texts[name]):
            report["why"] = f"request for {name} carries another query's text: the log and the driver are out of step"
            return report
    for (name, timed), r in zip(plan, reqs):
        if not timed: continue
        q = report["queries"].setdefault(name, {"runs": 0, "index": [0] * 5, "dictionary": [0] * 5})
        q["runs"] += 1
        for f, v in r["files"].items():
            part = "dictionary" if f in DICTIONARY else "index" if f in ORDERS else None
            if part: q[part] = [a + b for a, b in zip(q[part], v)]
    for q in report["queries"].values():
        for part in ("index", "dictionary"):
            h, m, calls, nbytes, ev = q[part]
            q[part] = {"hits": h, "misses": m, "hit_rate": round(h / (h + m), 4) if h + m else None,
                       "read_calls": calls, "bytes_read": nbytes, "evictions": ev}
    report["joined"] = True
    return report


if __name__ == "__main__":
    if len(sys.argv) != 4: sys.exit(__doc__)
    json.dump(main(*sys.argv[1:]), sys.stdout, indent=1)
    print()
