#!/usr/bin/env python3
"""Compare two arms of a benchmark (A = baseline, B = candidate), each given as one driver results file per server
session (`driver.py --out`), with the noise gate the team agreed in #105 and #130:

- Only eligible sessions count; an ineligible session makes the whole comparison refuse (exit 2).
- Every query must return the same answer (the driver's `bag`/`seq` hashes) in every session of both arms; a query
  whose answers differ is reported `answers differ` and the comparison refuses.
- Per query, each session contributes the median of its timed runs; an arm's value is the median of its session medians.
- `spread` is the larger arm's relative range of session medians (max/min - 1). A change counts only if B/A is outside
  [1/(1+spread), 1+spread] and beyond the keep threshold (default 3%); otherwise it is `noise`.
- Every arm needs at least 2 distinct sessions (distinct files, distinct run timestamps), and 3 when either arm's median
  is under 5 ms (Alex's agg-LUBM lesson: one binary swung 1.6x between server starts); with fewer a query is `too few
  sessions`, never a change.
- Answers compare by the driver's bag digest and row count; the row order (`seq`) only for ordered queries (those the
  driver marks with `order_matches_reference`). A missing digest or row count is not comparable.
- A duration that is zero, negative or not finite is not scorable.

Exit status: 0 comparable, 1 with --fail-on-regression and at least one counted slowdown, 2 refused.
Usage: abba.py --a A1.json A2.json ... --b B1.json B2.json ... [--threshold 0.03] [--json out.json]
       abba.py --self-test
"""
import argparse, json, math, statistics, sys

GOOD = {"pass", "value-compatible"}
FAST_S = 0.005
FAST_MIN_SESSIONS = 3
MIN_SESSIONS = 2


def load(paths):
    sessions = []
    for p in paths:
        with open(p) as f: sessions.append((p, json.load(f)))
    return sessions


def session_value(entry):
    """(median seconds, answer key) of one query in one session, or (None, reason)."""
    runs = [r for r in entry.get("runs", []) if r.get("class") in GOOD]
    if entry.get("class") not in GOOD or not runs: return None, entry.get("class", "missing")
    secs = [r.get("seconds") for r in runs]
    if any(not isinstance(x, (int, float)) or not math.isfinite(x) or x <= 0 for x in secs): return None, "unscorable duration"
    if any(not r.get("bag") or not isinstance(r.get("rows"), int) for r in runs): return None, "no answer digest"
    ordered = "order_matches_reference" in entry
    keys = {(r["bag"], r["rows"], r.get("seq") if ordered else None) for r in runs}
    if len(keys) != 1: return None, "unstable answer"
    return statistics.median(secs), keys.pop()


def session_id(path, s):
    """What makes two sessions distinct: the file and the wall-clock start of its first timed run."""
    starts = sorted(r.get("t0") for e in s.get("queries", {}).values() for r in e.get("runs", []) if r.get("t0") is not None)
    return starts[0] if starts else path


def compare(a_sessions, b_sessions, threshold=0.03):
    refused = []
    for arm, sessions in (("A", a_sessions), ("B", b_sessions)):
        paths = [p for p, _ in sessions]; ids = [session_id(p, s) for p, s in sessions]
        if len(set(paths)) != len(paths) or len(set(ids)) != len(ids): refused.append(f"arm {arm}: the same session is given more than once")
    refused += [f"{p}: ineligible ({'; '.join(s.get('ineligible_reasons', [])) or 'no reason recorded'})"
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
            need = FAST_MIN_SESSIONS if min(a, b) < FAST_S else MIN_SESSIONS
            if min(row["sessions"]) < need: row["verdict"] = "too few sessions"
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
    clock = iter(range(1, 10**6))
    def session(times, eligible=True, bag="h1", cls="pass", seq=None, ordered=False, rows=1):
        t = next(clock)
        return {"eligible": eligible, "ineligible_reasons": [] if eligible else ["q1: timeout"],
                "queries": {q: {"class": cls, **({"order_matches_reference": True} if ordered else {}),
                                "runs": [{"class": cls, "seconds": x, "t0": t + i / 100, "bag": bag, "seq": seq, "rows": rows} for i, x in enumerate(ts)]}
                            for q, ts in times.items()}}
    def arm(n, times, **kw): return [(f"s{next(clock)}", session(times, **kw)) for _ in range(n)]
    ok = True
    def check(name, cond):
        nonlocal ok; ok &= bool(cond); print(("ok  " if cond else "FAIL"), name)
    slow, fast = {"q": [0.100, 0.101, 0.099]}, {"q": [0.080, 0.081, 0.079]}
    r = compare(arm(2, slow), arm(2, fast))
    check("a 20% speedup, 2 tight sessions per arm: 'faster'", r["rows"][0]["verdict"] == "faster" and not r["refused"])
    r = compare([("a1", session({"q": [0.100]})), ("a2", session({"q": [0.130]}))], [("b1", session({"q": [0.110]})), ("b2", session({"q": [0.112]}))])
    check("a change inside the 30% session spread: 'noise'", r["rows"][0]["verdict"] == "noise")
    r = compare(arm(2, {"q": [0.100]}), arm(2, {"q": [0.101]}))
    check("1% is below the 3% keep threshold: 'noise'", r["rows"][0]["verdict"] == "noise")
    r = compare(arm(1, slow), arm(1, fast))
    check("one session per arm: 'too few sessions'", r["rows"][0]["verdict"] == "too few sessions")
    r = compare(arm(2, {"q": [0.006]}), arm(2, {"q": [0.004]}))
    check("6 -> 4 ms with 2 sessions per arm (B under 5 ms): 'too few sessions'", r["rows"][0]["verdict"] == "too few sessions")
    r = compare(arm(3, {"q": [0.002]}), arm(3, {"q": [0.001]}))
    check("under 5 ms with 3 distinct sessions per arm and no spread: counted", r["rows"][0]["verdict"] == "faster")
    one = session({"q": [0.002]})
    r = compare([("a", one), ("a", one), ("a", one)], arm(3, {"q": [0.001]}))
    check("the same session three times refuses", any("more than once" in x for x in r["refused"]))
    r = compare(arm(2, slow), arm(2, fast, bag="h2"))
    check("different answers between arms refuse", r["refused"] and r["rows"][0]["verdict"] == "answers differ")
    r = compare(arm(2, slow, bag=None), arm(2, fast, bag=None))
    check("missing answer digests are not comparable", r["refused"] and r["rows"][0]["verdict"] == "not comparable")
    r = compare(arm(2, slow, seq="ab"), arm(2, fast, seq="ba"))
    check("an unordered query ignores row order", not r["refused"] and r["rows"][0]["verdict"] == "faster")
    r = compare(arm(2, slow, seq="ab", ordered=True), arm(2, fast, seq="ba", ordered=True))
    check("an ordered query with a different order refuses", r["refused"] and r["rows"][0]["verdict"] == "answers differ")
    r = compare(arm(2, {"q": [0.0]}), arm(2, fast))
    check("a zero duration is not scorable", r["refused"] and r["rows"][0]["verdict"] == "not comparable")
    json.dumps(r, allow_nan=False)
    r = compare(arm(2, slow, eligible=False), arm(2, fast))
    check("an ineligible session refuses the comparison", any("ineligible" in x for x in r["refused"]))
    r = compare(arm(2, slow), arm(2, fast, cls="timeout"))
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
        with open(a.json, "w") as f: json.dump(result, f, indent=1, allow_nan=False)
    if result["refused"]: sys.exit(2)
    if a.fail_on_regression and any(r["verdict"] == "slower" for r in result["rows"]): sys.exit(1)


if __name__ == "__main__":
    main()
