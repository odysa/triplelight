#!/usr/bin/env python3
"""Write profile v4's plan files (one per suite, bench/eval/plan.py format) into OUT_DIR for this machine.

  make-plans.py OUT_DIR [--suite NAME ...] [--systems triplelight,qlever,virtuoso] [--runs 3] [--sessions 3]

Plans name absolute dataset and store paths (plan.py checks them against what systems.sh serves), so they are generated
per machine from $TOOLS (default <repo>/bench/tools) and never committed. Protocol as in profile v3: warm, 3 timed runs,
120 s timeout, seed 0, drain threshold 200000 rows; plus 3 server sessions per system (sequence AAA), which the noise
gate needs for queries under 5 ms.
"""
import argparse, json, os, pathlib, subprocess

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def version(system):
    if system == "triplelight":
        return subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"], capture_output=True, text=True, check=True).stdout.strip()
    pins = dict(l.split("=", 1) for l in (ROOT / "bench/eval/VERSIONS").read_text().splitlines() if "=" in l and not l.startswith("#"))
    return pins[f"{system.upper()}_IMAGE"].split()[0]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("out"); ap.add_argument("--suite", action="append")
    ap.add_argument("--systems", default="triplelight,qlever,virtuoso"); ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--sessions", type=int, default=3, help="server starts per system (the noise gate needs 3 for queries under 5 ms)")
    a = ap.parse_args()
    tools = pathlib.Path(os.environ.get("TOOLS") or ROOT / "bench" / "tools")
    out = pathlib.Path(a.out); out.mkdir(parents=True, exist_ok=True)
    systems = a.systems.split(",")
    for line in (HERE / "suites.tsv").read_text().splitlines():
        if line.startswith("#") or not line.strip(): continue
        suite, dataset, queries, reference, _ = line.split("\t")
        if a.suite and suite not in a.suite: continue
        plan = {"name": f"profile-v4-{suite}", "dataset": dataset, "queries": str(ROOT / queries), "reference": str(ROOT / reference),
                "datasets": [{"name": dataset, "path": str(tools / f"{dataset}.nt")}],
                "stores": [{"name": f"{s}-{dataset}", "path": str(tools / f"{s}-{dataset}")} for s in systems],
                "systems": [{"name": s, "version": version(s), "config": str(HERE / "configs" / f"{s}.txt")} for s in systems],
                "mode": "warm", "runs": a.runs, "timeout": 120, "seed": 0, "drain_rows": 200000, "sequence": "A" * a.sessions}
        (out / f"{suite}.json").write_text(json.dumps(plan, indent=1) + "\n")
        print(out / f"{suite}.json")


if __name__ == "__main__":
    main()
