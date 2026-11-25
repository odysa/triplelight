#!/usr/bin/env python3
"""Report and scoreboard for plan runs (task #130, phase 5).

  report.py report RUN_DIR [--out report.md] [--json report.json]
  report.py scoreboard add RUN_DIR --board FILE.jsonl      append one row per scored, eligible results file (idempotent by run id and system)
  report.py scoreboard show --board FILE.jsonl
  report.py selftest

RUN_DIR is a plan output directory (session-*/ with manifest.json and one <system>.<mode>.json per engine) or one session directory.
A results file contributes timings only if it says `scored: true`, its manifest exists and has the digest the results recorded, and the
run was eligible. Anything else is listed under "Not scored" with the reason and gets no timing row, so an ineligible or unscored run can
never reach a table or the scoreboard. Output names files by their name inside RUN_DIR, never by absolute path.
"""
import argparse, hashlib, json, pathlib, statistics, sys, tempfile

GOOD = ("pass", "value-compatible")


def sha256_file(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()


def session_dirs(run):
    run = pathlib.Path(run)
    if (run / "manifest.json").exists(): return [run]
    return sorted(d for d in run.glob("session-*") if d.is_dir())


def load_results(run):
    """Every results file under the run as (session name, system, mode, data, problems): `problems` is empty only for a scored, eligible file whose manifest matches."""
    out = []
    for sd in session_dirs(run):
        manifest = sd / "manifest.json"
        for f in sorted(sd.glob("*.json")):
            if f.name in ("manifest.json", "machine-check.json", "reference.json"): continue
            try: data = json.loads(f.read_text())
            except Exception as e: out.append((sd.name, f.stem, "?", {}, [f"{f.name}: not readable ({type(e).__name__})"])); continue
            if not isinstance(data, dict) or "queries" not in data: continue
            system, _, mode = f.stem.partition(".")
            arm = sd.name.rsplit("-", 1)[-1] if sd.name.startswith("session-") else ""
            problems = []
            if data.get("scored") is not True: problems.append("not scored" + (f": {data['scored_reason']}" if data.get("scored_reason") else ""))
            if data.get("eligible") is not True: problems.append("ineligible: " + "; ".join(data.get("ineligible_reasons") or ["unknown"]))
            if not manifest.exists(): problems.append("no manifest beside the results")
            elif data.get("manifest_sha256") != sha256_file(manifest): problems.append("the manifest does not match the digest the results recorded")
            if not problems:
                problems += inconsistent(data, system, json.loads(manifest.read_text()))
            data["_arm"] = arm
            out.append((sd.name, system, mode or data.get("mode", "?"), data, problems))
    return out


def inconsistent(data, system, manifest):
    """Problems that make a results file that claims to be scored and eligible unfit for publication: a query that is neither a good answer nor a
    declared unsupported case of this engine, or a good answer without a timed median. Their classes stay visible in the report's diagnostics."""
    declared = set((manifest.get("protocol") or {}).get("expect_unsupported", {}).get(system, []))
    bad = []
    for q, e in data["queries"].items():
        cls = e.get("class")
        if cls in GOOD:
            if "median_s" not in e: bad.append(f"{q}: {cls} without a timed median")
        elif not (cls == "unsupported" and q in declared): bad.append(f"{q}: {cls}" + (" (not declared as unsupported for this engine)" if cls == "unsupported" else ""))
    return ["internally inconsistent: marked eligible, but " + "; ".join(bad)] if bad else []


def ms(x): return f"{x * 1000:.2f}" if x < 1 else f"{x * 1000:.0f}"


def build(run):
    results = load_results(run)
    scored = [r for r in results if not r[4]]
    refused = [r for r in results if r[4]]
    lines = [f"# Benchmark report: {pathlib.Path(run).name}", ""]
    meta, arms = {}, {}
    for sd in session_dirs(run):
        m = json.loads((sd / "manifest.json").read_text()) if (sd / "manifest.json").exists() else {}
        arm = sd.name.rsplit("-", 1)[-1] if sd.name.startswith("session-") else ""
        ident = ((m.get("git") or {}).get("commit", "")[:12], ((m.get("build") or {}).get("binary_sha256") or "")[:12])
        arms.setdefault(arm, set()).add(ident)
        if not meta:
            meta = {"run_id": m.get("run_id"), "commit": (m.get("git") or {}).get("commit"), "machine": (m.get("machine") or {}).get("model"),
                    "os": (m.get("machine") or {}).get("os"), "mode": (m.get("protocol") or {}).get("mode"), "runs": (m.get("protocol") or {}).get("runs"),
                    "order": (m.get("protocol") or {}).get("order")}
    if meta:
        lines += [f"Run `{meta['run_id']}` on {meta['machine']} ({meta['os']}), {meta['mode']} mode, {meta['runs']} timed runs per query, order {meta['order']}.", ""]
        for arm, idents in sorted(arms.items()):
            lines.append(f"- {'arm ' + arm if arm else 'build'}: " + "; ".join(f"commit `{c}`, binary `{b}`" for c, b in sorted(idents)))
        lines.append("")
    multi = len([a for a in arms if a]) > 1
    table, relaxed = {}, {}
    for sess, system, mode, data, _ in scored:
        col = f"{system} ({data['_arm']})" if multi else system
        for q, e in data["queries"].items():
            table.setdefault((mode, q), {}).setdefault(col, []).append((sess, e))
            if e.get("class") == "value-compatible":
                relaxed.setdefault((col, mode, q), set()).update(e.get("policy_reasons") or ["narrower integer types normalized"])
    for mode in sorted({k[0] for k in table}):
        systems = sorted({s for (m, _), row in table.items() if m == mode for s in row})
        lines += [f"## {mode} (median seconds across the sessions of one arm, shown in milliseconds)", "", "| Query | " + " | ".join(systems) + " |", "| --- | " + " | ".join("---:" for _ in systems) + " |"]
        for (m, q), row in sorted(table.items(), key=lambda kv: (kv[0][1].rstrip("0123456789"), int("".join(c for c in kv[0][1] if c.isdigit()) or 0))):
            if m != mode: continue
            cells = []
            for s in systems:
                es = [e for _, e in row.get(s, [])]
                if not es: cells.append("-")
                elif all(e["class"] in GOOD and "median_s" in e for e in es): cells.append(ms(statistics.median(e["median_s"] for e in es)) + ("\\*" if any(e["class"] == "value-compatible" for e in es) else ""))
                else: cells.append("/".join(sorted({e["class"] for e in es})))      # a class, never a time
            lines.append(f"| {q} | " + " | ".join(cells) + " |")
        lines.append("")
    if relaxed:
        lines += ["## Relaxed comparisons (value-compatible, marked \\* above)", "", "The answer was accepted under a registered, weaker comparison than an exact match. Each reason is the policy's own.", ""]
        for (col, mode, q), reasons in sorted(relaxed.items()): lines.append(f"- {col}, {mode}, {q}: " + "; ".join(sorted(reasons)))
        lines.append("")
    if refused:
        lines += ["## Not scored", "", "These results get no timing row. Their classes and reasons are kept here for diagnosis.", ""]
        for sess, system, mode, _, problems in refused: lines.append(f"- `{sess}/{system}.{mode}`: " + "; ".join(problems))
        lines.append("")
    for f in sorted(pathlib.Path(run).glob("abba-*.json")):
        try: ab = json.loads(f.read_text())
        except Exception: continue
        rows = ab.get("rows") or []
        if ab.get("refused") or ab.get("comparable") is False:
            # a refused comparison publishes its reasons only: its provisional rows, ratios and verdicts are not results
            lines += [f"## A/B comparison: {f.stem[5:]}", "", "Refused. No ratio or verdict is published for this comparison.", ""] + [f"- {x}" for x in ab.get("refused") or ["the comparator marked this comparison not comparable"]] + [""]
            continue
        counts = {}
        for r in rows: counts[r.get("verdict", "?")] = counts.get(r.get("verdict", "?"), 0) + 1
        lines += [f"## A/B comparison: {f.stem[5:]}", "", "Verdicts: " + (", ".join(f"{k} {v}" for k, v in sorted(counts.items())) or "none") + f" over {len(rows)} queries; geometric mean B/A {ab.get('geomean_ratio')}.", ""]
        counted = [r for r in rows if r.get("verdict") in ("faster", "slower")]
        if counted:
            lines += ["| Query | A ms | B ms | B/A | spread | verdict |", "| --- | ---: | ---: | ---: | ---: | --- |"]
            lines += [f"| {r['query']} | {r['a_s'] * 1e3:.2f} | {r['b_s'] * 1e3:.2f} | {r['ratio']:.3f} | {r.get('spread', 0):.1%} | {r['verdict']} |" for r in counted]
            lines.append("")
    return "\n".join(lines) + "\n", {"meta": meta, "arms": {a: sorted(i) for a, i in arms.items()}, "scored": [(s, sy, m) for s, sy, m, _, _ in scored], "refused": [(s, sy, m, p) for s, sy, m, _, p in refused]}


def cmd_report(a):
    text, data = build(a.run)
    pathlib.Path(a.out).write_text(text) if a.out else print(text, end="")
    if a.json: pathlib.Path(a.json).write_text(json.dumps(data, indent=1, sort_keys=True) + "\n")
    if data["refused"] and not data["scored"]: sys.exit(1)       # nothing in this run may be published


def board_rows(run):
    rows = []
    for sess, system, mode, data, problems in load_results(run):
        if problems: continue
        meds = [e["median_s"] for e in data["queries"].values() if e.get("class") in GOOD and "median_s" in e]
        unsupported = sum(1 for e in data["queries"].values() if e.get("class") == "unsupported")      # declared for this engine, or the file would be refused above
        rows.append({"run_id": data.get("run_id"), "system": system, "mode": mode, "session": sess, "arm": data.get("_arm", ""), "queries": len(data["queries"]),
                     "passed": len(meds), "expected_unsupported": unsupported,
                     "geomean_s": round(statistics.geometric_mean(meds), 6) if meds else None, "manifest_sha256": data.get("manifest_sha256")})
    return rows


def cmd_board(a):
    board = pathlib.Path(a.board)
    existing = [json.loads(l) for l in board.read_text().splitlines() if l.strip()] if board.exists() else []
    if a.action == "show":
        for r in existing: print(f"{r['run_id']} {r['system']} {r['mode']} {r['session']}: {r['passed']} passed + {r.get('expected_unsupported', 0)} expected-unsupported of {r['queries']} queries, geomean {r['geomean_s']} s")
        return
    have = {(r["run_id"], r["system"], r["mode"], r["session"]) for r in existing}
    new = [r for r in board_rows(a.run) if (r["run_id"], r["system"], r["mode"], r["session"]) not in have]
    with board.open("a") as f:
        for r in new: f.write(json.dumps(r, sort_keys=True) + "\n")
    print(f"scoreboard: {len(new)} row(s) added, {len(board_rows(a.run)) - len(new)} already present")


def selftest(a):
    ok = True
    def check(label, good, detail=""):
        nonlocal ok
        print(f"report   {label:<70} {'ok' if good else 'FAIL ' + detail}"); ok &= bool(good)
    with tempfile.TemporaryDirectory() as t:
        t = pathlib.Path(t) / "run"; sd = t / "session-1-A"; sd.mkdir(parents=True)
        def manifest(d, commit, binary, expect=None):
            m = {"run_id": "r1", "git": {"commit": commit * 40}, "build": {"binary_sha256": binary * 64}, "machine": {"model": "box", "os": "linux"},
                 "protocol": {"mode": "warm", "runs": 5, "order": "ABBA", "expect_unsupported": expect or {}}}
            (d / "manifest.json").write_text(json.dumps(m)); return sha256_file(d / "manifest.json")
        h = manifest(sd, "a", "1")
        def res(d, h, system, **kw):
            data = {"system": system, "mode": "warm", "run_id": "r1", "scored": True, "eligible": True, "manifest_sha256": h,
                    "queries": {"q1": {"class": "pass", "median_s": 0.0123}, "q2": {"class": "pass", "median_s": 2.5}}}
            data.update(kw); (d / f"{system}.warm.json").write_text(json.dumps(data))
        res(sd, h, "fast"); res(sd, h, "unscored", scored=False, scored_reason="no manifest")
        res(sd, h, "wrong", eligible=False, ineligible_reasons=["q1: mismatch"], queries={"q1": {"class": "mismatch", "median_s": 0.001}})
        res(sd, h, "tampered", manifest_sha256="0" * 64)
        res(sd, h, "mixed", queries={"q1": {"class": "pass", "median_s": 0.004}, "q2": {"class": "mismatch", "median_s": 0.002}})        # claims eligible with a mismatch in it
        res(sd, h, "undeclared", queries={"q1": {"class": "pass", "median_s": 0.004}, "q2": {"class": "unsupported"}})
        text, data = build(t)
        table_part = text.split("## Not scored")[0]
        check("a scored, eligible result gets its timing rows", "| q1 |" in text and "12.30" in text and "2500" in text)
        check("an unscored result is listed as not scored, with the reason, and has no column or timing", "unscored.warm`: not scored: no manifest" in text and "unscored" not in table_part)
        check("an ineligible result is listed as not scored with its reasons", "wrong.warm`: ineligible: q1: mismatch" in text)
        check("a result whose manifest digest does not match is refused", "tampered.warm`: the manifest does not match" in text)
        check("a file marked eligible that holds a mismatch is refused as inconsistent, with the class kept", "mixed.warm`: internally inconsistent: marked eligible, but q2: mismatch" in text and "mixed" not in table_part)
        check("an unsupported answer not declared for that engine is refused as inconsistent", "undeclared.warm`: internally inconsistent: marked eligible, but q2: unsupported (not declared" in text)
        check("only the fast system appears as a column", "| Query | fast |" in text)
        check("no absolute path leaks into the report", "/tmp" not in text and "/var" not in text and str(t) not in text)
        check("the JSON lists scored and refused files separately", len(data["scored"]) == 1 and len(data["refused"]) == 5)
        board = t.parent / "board.jsonl"
        class A: pass
        b = A(); b.action = "add"; b.run = str(t); b.board = str(board); cmd_board(b); cmd_board(b)
        rows = [json.loads(l) for l in board.read_text().splitlines()]
        check("the scoreboard takes only scored, consistent, eligible results, once each", [r["system"] for r in rows] == ["fast"], str(rows))
        check("the scoreboard row has the denominator (passed, expected-unsupported, queries) and the manifest digest", rows[0]["passed"] == 2 and rows[0]["queries"] == 2 and rows[0]["expected_unsupported"] == 0 and rows[0]["manifest_sha256"] == h)
        # declared unsupported counts as its own denominator, not as a pass
        sd2 = t.parent / "run2" / "session-1-A"; sd2.mkdir(parents=True)
        h2 = manifest(sd2, "a", "1", {"qlever": ["q2"]})
        res(sd2, h2, "qlever", queries={"q1": {"class": "pass", "median_s": 0.01}, "q2": {"class": "unsupported"}})
        r2 = board_rows(t.parent / "run2")
        check("a declared unsupported case is its own count, outside the geometric mean", r2 and r2[0]["passed"] == 1 and r2[0]["expected_unsupported"] == 1 and r2[0]["queries"] == 2)
        # A/B runs: arms are not pooled, build identities are shown, and abba.py's own schema is read
        t3 = t.parent / "run3"
        for i, (arm, commit, binary, ms_) in enumerate((("A", "a", "1", 0.100), ("B", "b", "2", 0.020), ("B", "b", "2", 0.020), ("A", "a", "1", 0.100)), 1):
            d = t3 / f"session-{i}-{arm}"; d.mkdir(parents=True); hh = manifest(d, commit, binary)
            res(d, hh, "triplelight", queries={"q1": {"class": "pass", "median_s": ms_}})
        (t3 / "abba-triplelight.json").write_text(json.dumps({"geomean_ratio": 0.2, "refused": ["q9: answers differ between sessions or arms"],
                                                              "rows": [{"query": "q1", "a_s": 0.1, "b_s": 0.02, "ratio": 0.2, "spread": 0.01, "verdict": "faster"}, {"query": "q9", "verdict": "answers differ"}]}))
        text3, _ = build(t3)
        check("an A/B run shows each arm in its own column, never pooled", "| Query | triplelight (A) | triplelight (B) |" in text3 and "| q1 | 100.00 | 20.00 |" in text3, text3)
        check("an A/B run lists the commit and binary of each arm", "arm A: commit `aaaaaaaaaaaa`, binary `111111111111`" in text3 and "arm B: commit `bbbbbbbbbbbb`, binary `222222222222`" in text3)
        check("a refused comparison shows its reasons only: no verdict, ratio or geometric mean from its provisional rows", "answers differ between sessions or arms" in text3 and "faster 1" not in text3 and "0.200" not in text3 and "geometric mean" not in text3 and "No ratio or verdict" in text3)
        (t3 / "abba-triplelight.json").write_text(json.dumps({"geomean_ratio": 0.2, "refused": [], "rows": [{"query": "q1", "a_s": 0.1, "b_s": 0.02, "ratio": 0.2, "spread": 0.01, "verdict": "faster"}]}))
        (t3 / "abba-triplelight.json").write_text(json.dumps({"comparable": False, "geomean_ratio": None, "refused": ["engine configuration differs"], "rows": [{"query": "q1", "verdict": "withheld"}]}))
        text3c, _ = build(t3)
        check("a comparison the comparator marks comparable: false shows reasons only (the hardened schema)", "engine configuration differs" in text3c and "withheld" not in text3c and "geometric mean" not in text3c)
        (t3 / "abba-triplelight.json").write_text(json.dumps({"geomean_ratio": 0.2, "refused": [], "rows": [{"query": "q1", "a_s": 0.1, "b_s": 0.02, "ratio": 0.2, "spread": 0.01, "verdict": "faster"}]}))
        text3b, _ = build(t3)
        check("a comparable A/B comparison shows the verdict counts and the counted rows", "faster 1" in text3b and "| q1 | 100.00 | 20.00 | 0.200 |" in text3b and "geometric mean B/A 0.2" in text3b)
        # a relaxed comparison is never silent
        sd4 = t.parent / "run4" / "session-1-A"; sd4.mkdir(parents=True); h4 = manifest(sd4, "a", "1")
        res(sd4, h4, "ontop", queries={"q1": {"class": "value-compatible", "median_s": 0.01, "policy_reasons": ["within the registered binary32 interval"]}, "q2": {"class": "value-compatible", "median_s": 0.02}})
        text4, _ = build(t.parent / "run4")
        check("a value-compatible answer is marked and its reason is listed (the policy's own, or the default)", "\\*" in text4 and "q1: within the registered binary32 interval" in text4 and "q2: narrower integer types normalized" in text4)
        for f in sd.glob("*.json"):
            if f.name != "manifest.json": f.unlink()
        res(sd, h, "wrong", eligible=False, ineligible_reasons=["x"])
        try: cmd_report(type("B", (), {"run": str(t), "out": str(t.parent / "r.md"), "json": None})())
        except SystemExit as e: check("a run with nothing scored exits 1, and still writes what was refused", e.code == 1 and (t.parent / "r.md").exists())
        else: check("a run with nothing scored exits 1", False)
    print("REPORT SELFTEST", "PASSED" if ok else "FAILED"); sys.exit(0 if ok else 1)


def main():
    p = argparse.ArgumentParser(); sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("report"); r.add_argument("run"); r.add_argument("--out"); r.add_argument("--json"); r.set_defaults(f=cmd_report)
    b = sub.add_parser("scoreboard"); b.add_argument("action", choices=["add", "show"]); b.add_argument("run", nargs="?"); b.add_argument("--board", required=True); b.set_defaults(f=cmd_board)
    s = sub.add_parser("selftest"); s.set_defaults(f=selftest)
    a = p.parse_args(); a.f(a)


if __name__ == "__main__":
    main()
