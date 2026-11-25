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
            problems = []
            if data.get("scored") is not True: problems.append("not scored" + (f": {data['scored_reason']}" if data.get("scored_reason") else ""))
            if data.get("eligible") is not True: problems.append("ineligible: " + "; ".join(data.get("ineligible_reasons") or ["unknown"]))
            if not manifest.exists(): problems.append("no manifest beside the results")
            elif data.get("manifest_sha256") != sha256_file(manifest): problems.append("the manifest does not match the digest the results recorded")
            out.append((sd.name, system, mode or data.get("mode", "?"), data, problems))
    return out


def ms(x): return f"{x * 1000:.2f}" if x < 1 else f"{x * 1000:.0f}"


def build(run):
    results = load_results(run)
    scored = [r for r in results if not r[4]]
    refused = [r for r in results if r[4]]
    lines = [f"# Benchmark report: {pathlib.Path(run).name}", ""]
    meta = {}
    for sd in session_dirs(run)[:1]:
        m = json.loads((sd / "manifest.json").read_text()) if (sd / "manifest.json").exists() else {}
        meta = {"run_id": m.get("run_id"), "commit": (m.get("git") or {}).get("commit"), "machine": (m.get("machine") or {}).get("model"),
                "os": (m.get("machine") or {}).get("os"), "mode": (m.get("protocol") or {}).get("mode"), "runs": (m.get("protocol") or {}).get("runs"),
                "order": (m.get("protocol") or {}).get("order")}
        lines += [f"Run `{meta['run_id']}` at commit `{(meta['commit'] or '')[:12]}` on {meta['machine']} ({meta['os']}), {meta['mode']} mode, {meta['runs']} timed runs per query, order {meta['order']}.", ""]
    table = {}
    for sess, system, mode, data, _ in scored:
        for q, e in data["queries"].items():
            table.setdefault((mode, q), {}).setdefault(system, []).append((sess, e))
    for mode in sorted({k[0] for k in table}):
        systems = sorted({s for (m, _), row in table.items() if m == mode for s in row})
        lines += [f"## {mode} (median seconds across sessions, milliseconds)", "", "| Query | " + " | ".join(systems) + " |", "| --- | " + " | ".join("---:" for _ in systems) + " |"]
        for (m, q), row in sorted(table.items(), key=lambda kv: (kv[0][1].rstrip("0123456789"), int("".join(c for c in kv[0][1] if c.isdigit()) or 0))):
            if m != mode: continue
            cells = []
            for s in systems:
                es = [e for _, e in row.get(s, [])]
                if not es: cells.append("-")
                elif all(e["class"] in GOOD and "median_s" in e for e in es): cells.append(ms(statistics.median(e["median_s"] for e in es)))
                else: cells.append("/".join(sorted({e["class"] for e in es})))      # a class, never a time
            lines.append(f"| {q} | " + " | ".join(cells) + " |")
        lines.append("")
    if refused:
        lines += ["## Not scored", "", "These results get no timing row.", ""]
        for sess, system, mode, _, problems in refused: lines.append(f"- `{sess}/{system}.{mode}`: " + "; ".join(problems))
        lines.append("")
    for f in sorted(pathlib.Path(run).glob("abba-*.json")):
        try: ab = json.loads(f.read_text())
        except Exception: continue
        counts = {}
        for q in (ab.get("queries") or {}).values(): counts[q.get("verdict", q.get("status", "?"))] = counts.get(q.get("verdict", q.get("status", "?")), 0) + 1
        lines += [f"## A/B comparison: {f.stem[5:]}", "", "Verdicts: " + (", ".join(f"{k} {v}" for k, v in sorted(counts.items())) or "none recorded") + f" (from `{f.name}`).", ""]
    return "\n".join(lines) + "\n", {"meta": meta, "scored": [(s, sy, m) for s, sy, m, _, _ in scored], "refused": [(s, sy, m, p) for s, sy, m, _, p in refused]}


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
        rows.append({"run_id": data.get("run_id"), "system": system, "mode": mode, "session": sess, "queries": len(data["queries"]), "passed": len(meds),
                     "geomean_s": round(statistics.geometric_mean(meds), 6) if meds else None, "manifest_sha256": data.get("manifest_sha256")})
    return rows


def cmd_board(a):
    board = pathlib.Path(a.board)
    existing = [json.loads(l) for l in board.read_text().splitlines() if l.strip()] if board.exists() else []
    if a.action == "show":
        for r in existing: print(f"{r['run_id']} {r['system']} {r['mode']} {r['session']}: {r['passed']}/{r['queries']} queries, geomean {r['geomean_s']} s")
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
        man = {"run_id": "r1", "git": {"commit": "a" * 40}, "machine": {"model": "box", "os": "linux"}, "protocol": {"mode": "warm", "runs": 5, "order": "A"}}
        (sd / "manifest.json").write_text(json.dumps(man)); h = sha256_file(sd / "manifest.json")
        def res(system, **kw):
            d = {"system": system, "mode": "warm", "run_id": "r1", "scored": True, "eligible": True, "manifest_sha256": h,
                 "queries": {"q1": {"class": "pass", "median_s": 0.0123}, "q2": {"class": "pass", "median_s": 2.5}}}
            d.update(kw); (sd / f"{system}.warm.json").write_text(json.dumps(d))
        res("fast"); res("unscored", scored=False, scored_reason="no manifest"); res("wrong", eligible=False, ineligible_reasons=["q1: mismatch"],
                                                                                    queries={"q1": {"class": "mismatch", "median_s": 0.001}})
        res("tampered", manifest_sha256="0" * 64); res("mixed", queries={"q1": {"class": "pass", "median_s": 0.004}, "q2": {"class": "mismatch", "median_s": 0.002}})
        text, data = build(t)
        check("a scored, eligible result gets its timing rows", "| q1 |" in text and "12.30" in text and "2500" in text)
        table_part = text.split("## Not scored")[0]
        check("an unscored result is listed as not scored, with the reason, and has no column or timing", "unscored.warm`: not scored: no manifest" in text and "unscored" not in table_part)
        check("an ineligible result is listed as not scored with its reasons", "wrong.warm`: ineligible: q1: mismatch" in text)
        check("a result whose manifest digest does not match is refused", "tampered.warm`: the manifest does not match" in text)
        q2 = next(l for l in table_part.splitlines() if l.startswith("| q2 |"))
        check("a mismatching query in an otherwise scored file shows its class, never a time", q2 == "| q2 | 2500 | mismatch |", q2)
        check("only the fast and mixed systems appear as columns", "| Query | fast | mixed |" in text)
        check("no absolute path leaks into the report", "/tmp" not in text and "/var" not in text and str(t) not in text)
        check("the JSON lists scored and refused files separately", len(data["scored"]) == 2 and len(data["refused"]) == 3)
        board = t.parent / "board.jsonl"
        class A: pass
        b = A(); b.action = "add"; b.run = str(t); b.board = str(board); cmd_board(b); cmd_board(b)
        rows = [json.loads(l) for l in board.read_text().splitlines()]
        check("the scoreboard takes only scored, eligible results, once each", sorted(r["system"] for r in rows) == ["fast", "mixed"], str(rows))
        check("the scoreboard row records the manifest digest and a geometric mean", all(r["manifest_sha256"] == h and r["geomean_s"] for r in rows))
        for f in sd.glob("*.json"):
            if f.name != "manifest.json": f.unlink()
        res("wrong", eligible=False, ineligible_reasons=["x"])
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
