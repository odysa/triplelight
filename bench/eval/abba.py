#!/usr/bin/env python3
"""Compare two arms of a benchmark (A = baseline, B = candidate), each given as one driver results file per server
session (`driver.py --out`), with the noise gate the team agreed in #105 and #130:

- Only eligible sessions count; an ineligible session makes the whole comparison refuse (exit 2).
- Every query must return the same answer (the driver's `bag`/`seq` hashes) in every session of both arms; a query
  whose answers differ is reported `answers differ` and the comparison refuses.
- Per query, each session contributes the median of its timed runs; an arm's value is the median of its session medians.
- `spread` is the larger arm's relative range of session medians (max/min - 1). A change counts only if B/A is outside
  [1/(1+spread), 1+spread] and beyond the keep threshold (default 3%); otherwise it is `noise`.
- A query whose baseline median is under 5 ms needs at least 3 sessions per arm (Alex's agg-LUBM lesson: one binary
  swung 1.6x between server starts); with fewer it is `too few sessions`, never a change.

Exit status: 0 comparable, 1 with --fail-on-regression and at least one counted slowdown, 2 refused.
Usage: abba.py --a A1.json A2.json ... --b B1.json B2.json ... [--threshold 0.03] [--json out.json]
       abba.py --self-test
"""
import argparse, json, math, statistics, sys

GOOD = {"pass", "value-compatible"}
FAST_S = 0.005
FAST_MIN_SESSIONS = 3


def load(paths):
    sessions = []
    for p in paths:
        with open(p) as f: sessions.append((p, json.load(f)))
    return sessions


def session_value(entry):
    """(median seconds, answer key) of one query in one session, or (None, reason)."""
    runs = [r for r in entry.get("runs", []) if r.get("class") in GOOD and r.get("seconds") is not None]
    if entry.get("class") not in GOOD or not runs: return None, entry.get("class", "missing")
    keys = {(r.get("bag"), r.get("seq"), r.get("rows")) for r in runs}
    if len(keys) != 1: return None, "unstable answer"
    return statistics.median(r["seconds"] for r in runs), keys.pop()


def compare(a_sessions, b_sessions, threshold=0.03):
    refused = [f"{p}: ineligible ({'; '.join(s.get('ineligible_reasons', [])) or 'no reason recorded'})"
               for p, s in a_sessions + b_sessions if not s.get("eligible", False)]
    names = sorted(set().union(*(s["queries"].keys() for _, s in a_sessions + b_sessions)))
    rows = []
    for q in names:
        arm_vals, answers, problems = {"A": [], "B": []}, set(), []
        for arm, sessions in (("A", a_sessions), ("B", b_sessions)):
            for p, s in sessions:
                v, key = session_value(s["queries"].get(q, {}))
                if v is None: problems.append(f"{arm} {p}: {key}")
                else: arm_vals[arm].append(v); answers.add(key)
        row = {"query": q}
        if problems:
            row.update(verdict="not comparable", detail=problems); refused.append(f"{q}: " + "; ".join(problems))
        elif len(answers) != 1:
            row.update(verdict="answers differ"); refused.append(f"{q}: answers differ between sessions or arms")
        else:
            a, b = statistics.median(arm_vals["A"]), statistics.median(arm_vals["B"])
            spread = max(max(v) / min(v) - 1 if min(v) > 0 else math.inf for v in arm_vals.values())
            ratio = b / a if a > 0 else math.inf
            row.update(a_s=a, b_s=b, ratio=ratio, spread=spread, sessions=(len(arm_vals["A"]), len(arm_vals["B"])))
            if a < FAST_S and min(row["sessions"]) < FAST_MIN_SESSIONS: row["verdict"] = "too few sessions"
            elif ratio < min(1 / (1 + spread), 1 - threshold): row["verdict"] = "faster"
            elif ratio > max(1 + spread, 1 + threshold): row["verdict"] = "slower"
            else: row["verdict"] = "noise"
        rows.append(row)
    timed = [r["ratio"] for r in rows if "ratio" in r and r["ratio"] > 0 and math.isfinite(r["ratio"])]
    geomean = math.exp(sum(map(math.log, timed)) / len(timed)) if timed else None
    return {"refused": refused, "geomean_ratio": geomean, "rows": rows}


def render(result):
    out = [f"{'query':<24} {'A ms':>9} {'B ms':>9} {'B/A':>7} {'spread':>7}  verdict"]
    for r in result["rows"]:
        if "ratio" in r:
            out.append(f"{r['query']:<24} {r['a_s']*1e3:>9.2f} {r['b_s']*1e3:>9.2f} {r['ratio']:>7.3f} {r['spread']:>6.1%}  {r['verdict']}")
        else:
            out.append(f"{r['query']:<24} {'-':>9} {'-':>9} {'-':>7} {'-':>7}  {r['verdict']}")
    if result["geomean_ratio"] is not None: out.append(f"geomean B/A {result['geomean_ratio']:.3f}")
    for reason in result["refused"]: out.append(f"REFUSED: {reason}")
    return "\n".join(out)


def self_test():
    def session(times, eligible=True, bag="h1", cls="pass"):
        return {"eligible": eligible, "ineligible_reasons": [] if eligible else ["q1: timeout"],
                "queries": {q: {"class": cls, "runs": [{"class": cls, "seconds": t, "bag": bag, "seq": None, "rows": 1} for t in ts]}
                            for q, ts in times.items()}}
    ok = True
    def check(name, cond):
        nonlocal ok; ok &= bool(cond); print(("ok  " if cond else "FAIL"), name)
    slow = {"q": [0.100, 0.101, 0.099]}
    fast = {"q": [0.080, 0.081, 0.079]}
    r = compare([("a1", session(slow)), ("a2", session(slow))], [("b1", session(fast)), ("b2", session(fast))])
    check("a 20% speedup with tight sessions is 'faster'", r["rows"][0]["verdict"] == "faster" and not r["refused"])
    r = compare([("a1", session({"q": [0.100]})), ("a2", session({"q": [0.130]}))], [("b1", session({"q": [0.110]})), ("b2", session({"q": [0.112]}))])
    check("a change inside the 30% session spread is 'noise'", r["rows"][0]["verdict"] == "noise")
    r = compare([("a1", session({"q": [0.100]}))], [("b1", session({"q": [0.101]}))])
    check("1% is below the 3% keep threshold: 'noise'", r["rows"][0]["verdict"] == "noise")
    tiny_a, tiny_b = session({"q": [0.002]}), session({"q": [0.001]})
    r = compare([("a1", tiny_a), ("a2", tiny_a)], [("b1", tiny_b), ("b2", tiny_b)])
    check("under 5 ms with 2 sessions per arm: 'too few sessions'", r["rows"][0]["verdict"] == "too few sessions")
    r = compare([(f"a{i}", tiny_a) for i in range(3)], [(f"b{i}", tiny_b) for i in range(3)])
    check("under 5 ms with 3 sessions per arm and no spread: counted", r["rows"][0]["verdict"] == "faster")
    r = compare([("a1", session(slow))], [("b1", session(fast, bag="h2"))])
    check("different answers between arms refuse", r["refused"] and r["rows"][0]["verdict"] == "answers differ")
    r = compare([("a1", session(slow, eligible=False))], [("b1", session(fast))])
    check("an ineligible session refuses the comparison", any("ineligible" in x for x in r["refused"]))
    r = compare([("a1", session(slow))], [("b1", session(fast, cls="timeout"))])
    check("a timed-out query is not comparable and refuses", r["refused"] and r["rows"][0]["verdict"] == "not comparable")
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--a", nargs="+"); ap.add_argument("--b", nargs="+")
    ap.add_argument("--threshold", type=float, default=0.03)
    ap.add_argument("--json"); ap.add_argument("--fail-on-regression", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test: sys.exit(self_test())
    if not a.a or not a.b: ap.error("--a and --b are required")
    result = compare(load(a.a), load(a.b), a.threshold)
    print(render(result))
    if a.json:
        with open(a.json, "w") as f: json.dump(result, f, indent=1)
    if result["refused"]: sys.exit(2)
    if a.fail_on_regression and any(r["verdict"] == "slower" for r in result["rows"]): sys.exit(1)


if __name__ == "__main__":
    main()
