#!/usr/bin/env python3
"""One-command benchmark plan runner (task #130, phase 3).

  plan.py run PLAN.json --out DIR [--receipt machine-check.json] [--dry-run]
  plan.py selftest

A plan is a JSON file naming everything a run needs. The runner takes the machine lock for the whole plan, requires a fresh passing
machine-check receipt, writes a manifest for each session, runs each session through session.sh with SCORED=1 (so every results file
is bound to its manifest), and for cold mode invokes this repository's `restart.sh --cold` itself with exact arguments. Nothing is
timed unless every check passes; every session's logs and results are kept under DIR whatever happens.

Plan keys (all required unless marked optional):
  name, dataset (the name session.sh and systems.sh know), queries (directory of .rq files), reference (reference answers, driver.py reference),
  datasets [{name, path, sha256?}], stores [{name, path}] or "no_store": true,
  systems [{name, version, config}]  (the endpoint and cache-clear URL come from lib.sh, so the plan cannot disagree with session.sh),
  mode warm|cold, runs, timeout, seed, drain_rows,
  expect_unsupported (optional) {system: [case ids]},
  arms (optional) [{label, binary}]  and  sequence (optional, e.g. "ABBA"): TRIPLELIGHT_BIN per arm; default one arm "A", sequence "A".
  fail_on_regression (optional, default false): with exactly two arms, abba.py --scored compares them per system and a counted slowdown fails the plan.
"""
import argparse, hashlib, json, os, pathlib, re, shutil, subprocess, sys, tempfile, time

HERE = pathlib.Path(__file__).resolve().parent
LOCK = pathlib.Path(os.environ.get("PLAN_LOCK", "/tmp/rdf-bench.lock"))      # PLAN_LOCK is for the self-test only: it must never touch the real queue
KEYS = {"name", "dataset", "queries", "reference", "datasets", "stores", "no_store", "systems", "mode", "runs", "timeout", "seed", "drain_rows",
        "expect_unsupported", "arms", "sequence", "fail_on_regression"}
REQUIRED = {"name", "dataset", "queries", "reference", "datasets", "systems", "mode", "runs", "timeout", "seed", "drain_rows"}


class PlanError(Exception): pass


def shell_fn(function, system):
    """Calls a lib.sh function with the system name as a positional argument, never interpolated into the shell text."""
    return subprocess.run(["bash", "-c", '. "$1"; "$2" "$3"', "_", str(HERE / "lib.sh"), function, system], capture_output=True, text=True)


def endpoint(system):
    r = shell_fn("url_of", system)
    if r.returncode != 0: raise PlanError(f"unknown system {system!r}: lib.sh has no endpoint for it")
    return r.stdout.strip(), shell_fn("clear_cache_url_of", system).stdout.strip()


def load_plan(path):
    plan = json.loads(pathlib.Path(path).read_text())
    if not isinstance(plan, dict): raise PlanError("a plan is a JSON object")
    unknown = set(plan) - KEYS
    if unknown: raise PlanError(f"unknown plan keys: {sorted(unknown)}")
    missing = REQUIRED - set(plan)
    if missing: raise PlanError(f"missing plan keys: {sorted(missing)}")
    if not re.fullmatch(r"[A-Za-z0-9._-]+", str(plan["name"])): raise PlanError("name may use letters, digits, . _ - only")
    if plan["mode"] not in ("warm", "cold"): raise PlanError("mode is warm or cold")
    for k in ("runs", "seed", "drain_rows"):
        if not isinstance(plan[k], int) or isinstance(plan[k], bool): raise PlanError(f"{k} must be an integer")
    if plan["runs"] < 1: raise PlanError("runs must be at least 1")
    if not isinstance(plan["timeout"], (int, float)) or plan["timeout"] <= 0: raise PlanError("timeout must be positive")
    if not plan["systems"] or not plan["datasets"]: raise PlanError("systems and datasets cannot be empty")
    for s in plan["systems"]:
        if not {"name", "version", "config"} <= set(s): raise PlanError(f"system {s!r} needs name, version and config")
    if not plan.get("no_store") and not plan.get("stores"): raise PlanError('give "stores" (every persisted store the systems read) or "no_store": true')
    arms = plan.get("arms") or [{"label": "A", "binary": ""}]
    labels = [a["label"] for a in arms]
    if len(set(labels)) != len(labels) or not all(re.fullmatch(r"[A-Z]", l) for l in labels): raise PlanError("arm labels are distinct single capital letters")
    seq = plan.get("sequence") or labels[0]
    if set(seq) - set(labels): raise PlanError(f"sequence {seq!r} uses arms not defined: {sorted(set(seq) - set(labels))}")
    if len(labels) > 1 and not all(seq.count(l) == seq.count(labels[0]) for l in labels): raise PlanError("every arm appears equally often in the sequence")
    plan["arms"], plan["sequence"] = arms, seq
    for key in ("queries", "reference"):
        if not pathlib.Path(plan[key]).exists(): raise PlanError(f"{key} {plan[key]} does not exist")
    for d in plan["datasets"]:
        if not pathlib.Path(d["path"]).is_file(): raise PlanError(f"dataset {d['name']}: {d['path']} is not a file")
    for s in plan["systems"]:
        if not pathlib.Path(s["config"]).is_file(): raise PlanError(f"system {s['name']}: config {s['config']} is not a file")
    for a in plan["arms"]:
        if a.get("binary") and not pathlib.Path(a["binary"]).is_file(): raise PlanError(f"arm {a['label']}: binary {a['binary']} is not a file")
    return plan


def tools_dir():
    """Where systems.sh looks: $TOOLS, else <repo>/bench/tools (systems.sh: T=${TOOLS:-$ROOT/bench/tools}). Datasets are $T/<dataset>.nt and a system's store is $T/<system>-<dataset>."""
    return pathlib.Path(os.environ.get("TOOLS") or HERE.parents[1] / "bench" / "tools")


def check_session_inputs(plan):
    """The manifest must hash what the session will actually load and serve, so the plan's dataset and store paths must be the ones systems.sh derives."""
    t, d = tools_dir(), plan["dataset"]
    want = os.path.realpath(t / f"{d}.nt")
    named = [x for x in plan["datasets"] if x["name"] == d]
    if len(named) != 1 or os.path.realpath(named[0]["path"]) != want:
        raise PlanError(f"dataset {d!r}: the session loads {want}; the plan must list exactly that file under the dataset's name")
    if len(plan["datasets"]) != 1: raise PlanError("a session loads one dataset: list only the plan's dataset")
    if not plan.get("no_store"):
        expected = {os.path.realpath(t / f"{s['name']}-{d}") for s in plan["systems"]}
        listed = {os.path.realpath(x["path"]) for x in plan.get("stores", [])}
        if listed != expected: raise PlanError(f"stores: the session serves {sorted(expected)}; the plan lists {sorted(listed)}")


def manifest_command(plan, arm, receipt, out, reference=None, plan_file=None):
    cmd = [sys.executable, str(HERE / "manifest.py"), "write", "--out", str(out), "--receipt", str(receipt), "--queries", plan["queries"],
           "--reference", str(reference or plan["reference"]), "--mode", plan["mode"], "--runs", str(plan["runs"]), "--timeout", str(plan["timeout"]),
           "--seed", str(plan["seed"]), "--drain-rows", str(plan["drain_rows"]), "--order", plan["sequence"], "--run-id", f"{plan['name']}-{out.name}"]
    if plan_file: cmd += ["--plan", str(plan_file)]
    cmd += ["--binary", arm.get("binary") or os.environ.get("TRIPLELIGHT_BIN") or str(HERE.parents[1] / "target" / "release" / "triplelight")]
    for d in plan["datasets"]: cmd += ["--dataset", f"{d['name']}:{d['path']}" + (f":{d['sha256']}" if d.get("sha256") else "")]
    for st in plan.get("stores", []): cmd += ["--store", f"{st['name']}:{st['path']}"]
    if plan.get("no_store"): cmd += ["--no-store"]
    for s in plan["systems"]:
        url, clear = endpoint(s["name"])
        cmd += ["--system", f"{s['name']}:{s['version']}:{s['config']}", "--system-url", f"{s['name']}={url}"]
        if clear: cmd += ["--system-clear-cache", f"{s['name']}={clear}"]
    for system, cases in sorted(plan.get("expect_unsupported", {}).items()):
        if cases: cmd += ["--expect-unsupported", f"{system}={','.join(cases)}"]
    if plan["mode"] == "cold": cmd += ["--cold-hook", f"'{HERE / 'restart.sh'}' --cold"]
    if os.environ.get("PLAN_TEST_ALLOW_UNQUALIFIED") == "1": cmd += ["--allow-unqualified"]
    return cmd


def session_env(plan, arm, manifest):
    env = dict(os.environ, MANIFEST=str(manifest), SCORED="1", MODE=plan["mode"], RUNS=str(plan["runs"]), TIMEOUT=str(plan["timeout"]),
               SEED=str(plan["seed"]), DRAIN_ROWS=str(plan["drain_rows"]), PLAN_RUNNER=json.loads(manifest.read_text())["run_id"] if manifest.exists() else "")
    if arm.get("binary"): env["TRIPLELIGHT_BIN"] = arm["binary"]
    for system, cases in plan.get("expect_unsupported", {}).items():
        env["EXPECT_UNSUPPORTED_" + re.sub(r"-", "_", system).upper()] = ",".join(cases)
    return env


def take_lock(what):
    try: LOCK.mkdir()
    except FileExistsError:
        owner = (LOCK / "owner").read_text().strip() if (LOCK / "owner").exists() else "unknown"
        sys.exit(f"the machine lock is held by: {owner}")        # exit status 1; the queue protocol is the caller's business
    (LOCK / "owner").write_text(what + "\n")


def release_lock(what):
    try:
        if (LOCK / "owner").read_text().strip() == what: subprocess.run(["rm", "-rf", str(LOCK)])
    except OSError: pass


def compare_arms(plan, out, sessions):
    """A/B plan: abba.py --scored per system, A (the first arm) the baseline and B the candidate. Exit 2 (refused) fails the plan; a counted
    slowdown fails it only with fail_on_regression. The comparison JSON is kept beside the sessions."""
    a_label, b_label = (x["label"] for x in plan["arms"])
    status = 0
    for system in [s["name"] for s in plan["systems"]]:
        files = {l: [str(out / f"session-{i}-{l}" / f"{system}.{plan['mode']}.json") for i, lab, _ in sessions if lab == l] for l in (a_label, b_label)}
        cmd = [sys.executable, str(HERE / "abba.py"), "--scored", "--a", *files[a_label], "--b", *files[b_label], "--json", str(out / f"abba-{system}.json")]
        if plan.get("fail_on_regression"): cmd.append("--fail-on-regression")
        r = subprocess.run(cmd, capture_output=True, text=True)
        (out / f"abba-{system}.log").write_text(r.stdout + r.stderr)
        print(f"abba {system}: exit {r.returncode}")
        if r.returncode != 0: status = 1
    return status


def cmd_run(a):
    plan = load_plan(a.plan)
    check_session_inputs(plan)
    out = pathlib.Path(a.out)
    receipt = pathlib.Path(a.receipt or out / "machine-check.json")
    sessions = [(i + 1, label, next(x for x in plan["arms"] if x["label"] == label)) for i, label in enumerate(plan["sequence"])]
    print(f"plan {plan['name']}: {len(sessions)} session(s) {plan['sequence']} on dataset {plan['dataset']}, mode {plan['mode']}, systems {[s['name'] for s in plan['systems']]}")
    if a.dry_run:
        for i, label, arm in sessions:
            sdir = out / f"session-{i}-{label}"
            print(f"  [{i}] arm {label}: manifest.py write ... --out {sdir}")
            print(f"      session.sh {plan['dataset']} {plan['queries']} {sdir} {' '.join(s['name'] for s in plan['systems'])}  (SCORED=1)")
        print("dry run: nothing was started")
        return
    if out.exists() and any(out.iterdir()): sys.exit(f"{out} is not empty: a plan run never overwrites results")
    owner = f"plan {plan['name']} ({os.getpid()})"
    take_lock(owner)            # before the machine check: the check reads the machine's idle state, which is only meaningful while we own it
    try:
        out.mkdir(parents=True, exist_ok=True)
        if not a.receipt:
            mc = HERE / "machine-check.sh"
            if not mc.exists(): sys.exit("no machine receipt: pass --receipt, or add bench/eval/machine-check.sh (task #130, phase 6)")
            r = subprocess.run([str(mc), str(out)])          # machine-check.sh <output-dir> writes <output-dir>/machine-check.json
            if r.returncode != 0 or not receipt.exists(): sys.exit(f"machine check failed (exit {r.returncode}); nothing was run")
        status = 0
        for i, label, arm in sessions:
            sdir = out / f"session-{i}-{label}"
            sdir.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(plan["reference"], sdir / "reference.json")      # session.sh reads $OUT/reference.json: the very bytes the manifest hashes
            m = subprocess.run(manifest_command(plan, arm, receipt, sdir, sdir / "reference.json", a.plan), capture_output=True, text=True)
            (out / f"session-{i}-{label}.manifest.log").write_text(m.stdout + m.stderr)
            if m.returncode != 0: print(f"session {i} ({label}): manifest refused: {m.stderr.strip()[-400:]}", file=sys.stderr); status = 1; break
            script = os.environ.get("PLAN_SESSION_SCRIPT") or str(HERE / "session.sh")
            r = subprocess.run([script, plan["dataset"], plan["queries"], str(sdir), *[s["name"] for s in plan["systems"]]], env=session_env(plan, arm, sdir / "manifest.json"))
            print(f"session {i} ({label}): exit {r.returncode}")
            if r.returncode != 0: status = 1          # keep going: every session's logs are evidence; the plan's status says it failed
        (out / "plan.json").write_text(json.dumps(plan, indent=1, sort_keys=True))
        if len(plan["arms"]) == 2 and status == 0 and not os.environ.get("PLAN_SKIP_COMPARE"): status = compare_arms(plan, out, sessions)      # the skip is for the stub-session test
        sys.exit(status)
    finally:
        release_lock(owner)


def cmd_selftest(a):
    ok = True
    def check(label, good, detail=""):
        nonlocal ok
        print(f"plan     {label:<66} {'ok' if good else 'FAIL ' + detail}"); ok &= bool(good)
    with tempfile.TemporaryDirectory() as t:
        t = pathlib.Path(t)
        (t / "q").mkdir(); (t / "q" / "q1.rq").write_text("SELECT * WHERE {}"); (t / "ref.json").write_text("{}")
        tools0 = t / "tools0"; (tools0 / "triplelight-lubm1").mkdir(parents=True); (tools0 / "triplelight-lubm1" / "f").write_text("s"); (tools0 / "lubm1.nt").write_text("<a> <b> <c> .\n")
        (t / "cfg").write_text("x"); (t / "bin").write_text("b")
        base = {"name": "t1", "dataset": "lubm1", "queries": str(t / "q"), "reference": str(t / "ref.json"), "datasets": [{"name": "lubm1", "path": str(tools0 / "lubm1.nt")}],
                "stores": [{"name": "lubm1", "path": str(tools0 / "triplelight-lubm1")}], "systems": [{"name": "triplelight", "version": "x", "config": str(t / "cfg")}],
                "mode": "warm", "runs": 3, "timeout": 30, "seed": 0, "drain_rows": 1000}
        def plan_file(**kw):
            p = {**base, **kw}
            p = {k: v for k, v in p.items() if v is not None}
            f = t / "plan.json"; f.write_text(json.dumps(p)); return f
        def refuses(label, word, **kw):
            try: load_plan(plan_file(**kw)); check(label, False, "accepted")
            except PlanError as e: check(label, word in str(e), str(e))
        os.environ["TOOLS"] = str(tools0)           # where systems.sh would look for this plan's dataset and store
        check("a complete plan loads", bool(load_plan(plan_file())["sequence"] == "A"))
        def refuses_inputs(label, word, **kw):
            try: check_session_inputs(load_plan(plan_file(**kw))); check(label, False, "accepted")
            except PlanError as e: check(label, word in str(e), str(e))
        check_session_inputs(load_plan(plan_file())); check("the plan's dataset and store are the ones the session serves", True)
        (t / "elsewhere.nt").write_text("<x> <y> <z> .\n"); (t / "elsewhere").mkdir(); (t / "elsewhere" / "f").write_text("e")
        refuses_inputs("a dataset path that is not the session's file is refused", "the session loads", datasets=[{"name": "lubm1", "path": str(t / "elsewhere.nt")}])
        refuses_inputs("a store path that is not the session's store is refused", "the session serves", stores=[{"name": "lubm1", "path": str(t / "elsewhere")}])
        refuses_inputs("an extra dataset is refused", "one dataset", datasets=[{"name": "lubm1", "path": str(tools0 / "lubm1.nt")}, {"name": "other", "path": str(t / "elsewhere.nt")}])
        refuses("an unknown key is refused", "unknown plan keys", bogus=1)
        refuses("a missing key is refused", "missing plan keys", runs=None)
        refuses("zero runs are refused", "at least 1", runs=0)
        refuses("a bad mode is refused", "warm or cold", mode="hot")
        refuses("no stores and no no_store is refused", "no_store", stores=None)
        refuses("a missing queries directory is refused", "does not exist", queries=str(t / "nope"))
        refuses("a missing dataset file is refused", "not a file", datasets=[{"name": "x", "path": str(t / "nope")}])
        refuses("an unbalanced ABBA is refused", "equally often", arms=[{"label": "A", "binary": str(t / "bin")}, {"label": "B", "binary": str(t / "bin")}], sequence="ABB")
        refuses("a sequence with an undefined arm is refused", "not defined", sequence="AC")
        ab = load_plan(plan_file(arms=[{"label": "A", "binary": str(t / "bin")}, {"label": "B", "binary": str(t / "bin")}], sequence="ABBA"))
        check("an ABBA plan expands to four sessions", len(ab["sequence"]) == 4)
        env = session_env(ab, ab["arms"][1], t / "nomanifest.json")
        check("session env: SCORED, the arm's binary", env["SCORED"] == "1" and env["TRIPLELIGHT_BIN"] == str(t / "bin"))
        pl = load_plan(plan_file(expect_unsupported={"qlever-two-order": ["q3", "q7"]}))
        check("expected-unsupported is scoped to its own system", session_env(pl, pl["arms"][0], t / "m.json")["EXPECT_UNSUPPORTED_QLEVER_TWO_ORDER"] == "q3,q7")
        cmd = manifest_command(load_plan(plan_file(mode="cold")), {"label": "A", "binary": str(t / "bin")}, t / "r.json", t / "out")
        check("a cold plan records this repository's restart.sh --cold as the hook", f"{HERE / 'restart.sh'}' --cold" in " ".join(cmd))
        # dry run starts nothing and takes no lock
        r = subprocess.run([sys.executable, __file__, "run", str(plan_file()), "--out", str(t / "o1"), "--dry-run"], capture_output=True, text=True, env=dict(os.environ, TOOLS=str(tools0)))
        check("--dry-run prints the sessions, starts nothing and creates no output", r.returncode == 0 and "nothing was started" in r.stdout and not (t / "o1").exists(), r.stderr[-200:])
        # a real run with a stub session script: lock taken, manifest written per session, session invoked with SCORED
        stub = t / "stub.sh"
        stub.write_text('#!/bin/bash\necho "$@ SCORED=$SCORED MANIFEST=$MANIFEST BIN=${TRIPLELIGHT_BIN:-} LOCK=$(ls -d "$PLAN_LOCK" 2>&1)" >> "$STUB_LOG"\nexit "${STUB_EXIT:-0}"\n'); stub.chmod(0o755)
        rec = {"passed": True, "quick": False, "boot_id": subprocess.run([sys.executable, "-c", f"import sys; sys.path.insert(0, '{HERE}'); import manifest; print(manifest.boot_id())"], capture_output=True, text=True).stdout.strip(),
               "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "failures": [], **{f: "x" for f in ("model", "cpu", "cores", "ram_gb", "os", "kernel")}}
        (t / "rec.json").write_text(json.dumps(rec))
        lock = t / "lock"            # a private lock: the self-test never touches the shared machine lock
        env = dict(os.environ, PLAN_SESSION_SCRIPT=str(stub), STUB_LOG=str(t / "stub.log"), PLAN_TEST_ALLOW_UNQUALIFIED="1", PLAN_LOCK=str(lock), PLAN_SKIP_COMPARE="1", TOOLS=str(tools0))
        if True:
            r = subprocess.run([sys.executable, __file__, "run", str(plan_file(arms=[{"label": "A", "binary": str(t / "bin")}, {"label": "B", "binary": str(t / "bin")}], sequence="ABBA")),
                                "--out", str(t / "o2"), "--receipt", str(t / "rec.json")], capture_output=True, text=True, env=env)
            log = (t / "stub.log").read_text().splitlines() if (t / "stub.log").exists() else []
            check("a live run starts one session per entry of the sequence, SCORED=1", r.returncode == 0 and len(log) == 4 and all("SCORED=1" in l for l in log), r.stdout[-200:] + r.stderr[-300:])
            check("the machine lock is held during a session and released after", all(f"LOCK={lock}" in l for l in log) and not lock.exists())
            check("each session got its own manifest", all((t / "o2" / f"session-{i}-{l}" / "manifest.json").exists() for i, l in enumerate("ABBA", 1)))
            r = subprocess.run([sys.executable, __file__, "run", str(plan_file()), "--out", str(t / "o2"), "--receipt", str(t / "rec.json")], capture_output=True, text=True, env=env)
            check("an output directory with results is never overwritten", r.returncode != 0 and "not empty" in r.stderr)
            r = subprocess.run([sys.executable, __file__, "run", str(plan_file()), "--out", str(t / "o3"), "--receipt", str(t / "rec.json")], capture_output=True, text=True, env={**env, "STUB_EXIT": "3"})
            check("a failing session fails the plan (exit 1) and still releases the lock", r.returncode == 1 and not lock.exists())
            lock.mkdir(); (lock / "owner").write_text("someone else\n")
            try:
                r = subprocess.run([sys.executable, __file__, "run", str(plan_file()), "--out", str(t / "o4"), "--receipt", str(t / "rec.json")], capture_output=True, text=True, env=env)
                check("a held machine lock refuses the plan, naming the owner, and leaves it alone", r.returncode != 0 and "someone else" in r.stderr and lock.exists())
            finally: subprocess.run(["rm", "-rf", str(lock)])

        # The real plan-to-session wiring: plan.py -> manifest.py -> session.sh -> systems.sh -> driver.py -> restart.sh, with only the engine and the
        # host-touching reset stubbed. The self-test runs a COPY of bench/eval in the temporary directory, patched so that it can never reach the
        # shared machine: the engine's port is a free private one (not the shared service port), and `drop_caches` is a stub (the real one purges the
        # host's page cache when permissions allow). Nothing else is replaced: the copy is the same plan, session, driver and reset scripts.
        import shutil, socket
        ev = t / "eval"; shutil.copytree(HERE, ev, ignore=shutil.ignore_patterns("__pycache__"))
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0)); port = probe.getsockname()[1]
        for name in ("lib.sh", "systems.sh"):
            text = (ev / name).read_text()
            if name == "lib.sh":
                text = text.replace("7979", str(port))
                text, n = re.subn(r"(?ms)^drop_caches\(\) \{.*?^\}\n", 'drop_caches() { echo "page cache drop stubbed in the self-test"; }\n', text)
                check("the copy's drop_caches is stubbed (it never touches the host's page cache)", n == 1 and "purge" not in text.split("drop_caches()")[1].split("\n")[0])
            else: text = text.replace("7979", str(port))
            (ev / name).write_text(text)
        sys.path.insert(0, str(ev)); import driver as drv
        doc = '{"head":{"vars":["n"]},"results":{"bindings":[{"n":{"type":"literal","value":"9","datatype":"' + drv.XSD + 'integer"}}]}}'
        (t / "wref.json").write_text(json.dumps({"queries": {"q1": {**drv.fold_of([doc]), "ordered": False}}}))
        tools = t / "tools"; (tools / "triplelight-wire").mkdir(parents=True); (tools / "wire.nt").write_text("<a> <b> <c> .\n")
        fake = t / "fake-triplelight"
        fake.write_text("#!/usr/bin/env python3\nimport http.server, sys\n"
                        "if len(sys.argv) < 4 or sys.argv[2] == '/nonexistent': sys.exit(0)\n"
                        f"BODY = {doc!r}.encode()\n"
                        "class H(http.server.BaseHTTPRequestHandler):\n"
                        "    def log_message(self, *a): pass\n"
                        "    def do_POST(self):\n"
                        "        self.rfile.read(int(self.headers.get('Content-Length', 0))); self.send_response(200); self.send_header('Content-Type', 'application/sparql-results+json'); self.end_headers(); self.wfile.write(BODY)\n"
                        f"http.server.ThreadingHTTPServer(('127.0.0.1', {port}), H).serve_forever()\n")
        fake.chmod(0o755)
        repo = t / "cleanrepo"; repo.mkdir()
        for cmd in (["init", "-q"], ["config", "user.email", "t@example.org"], ["config", "user.name", "t"]): subprocess.run(["git", "-C", str(repo), *cmd], check=True)
        (repo / "f").write_text("x"); subprocess.run(["git", "-C", str(repo), "add", "f"], check=True); subprocess.run(["git", "-C", str(repo), "commit", "-qm", "c"], check=True)
        wenv = dict(os.environ, PLAN_LOCK=str(t / "wlock"), MANIFEST_REPO_ROOT=str(repo), TOOLS=str(tools), TRIPLELIGHT_BIN=str(fake))
        for k in ("PLAN_SESSION_SCRIPT", "PLAN_TEST_ALLOW_UNQUALIFIED", "PLAN_SKIP_COMPARE"): wenv.pop(k, None)
        wire = lambda **kw: plan_file(dataset="wire", reference=str(t / "wref.json"), datasets=[{"name": "wire", "path": str(tools / "wire.nt")}],
                                      stores=[{"name": "wire", "path": str(tools / "triplelight-wire")}], runs=2, drain_rows=200000, seed=5, **kw)
        (tools / "triplelight-wire" / "f").write_text("s")
        runplan = lambda plan, out: subprocess.run([sys.executable, str(ev / "plan.py"), "run", str(plan), "--out", str(out), "--receipt", str(t / "rec.json")], capture_output=True, text=True, env=wenv)
        pf1 = wire(); h1 = hashlib.sha256(pf1.read_bytes()).hexdigest()          # plan.json is rewritten by later calls: hash it now
        r = runplan(pf1, t / "w1")
        res = t / "w1" / "session-1-A" / "triplelight.warm.json"
        data = json.loads(res.read_text()) if res.exists() else {}
        check("real wiring, warm: the session runs, the results are scored: true, no failures", r.returncode == 0 and data.get("scored") is True and data.get("eligible") is True and not (t / "w1" / "session-1-A" / "failures.txt").exists(), r.stdout[-300:] + r.stderr[-500:])
        check("real wiring, warm: the plan's seed and repeat count reach the driver", data.get("seed") == 5 and data.get("runs_per_query") == 2)
        check("real wiring, warm: the reference staged for the session is the plan's file", (t / "w1" / "session-1-A" / "reference.json").read_bytes() == (t / "wref.json").read_bytes())
        check("real wiring, warm: the endpoint is the private port, not the shared service port", f":{port}/" in data.get("url", "") and ":7979/" not in data.get("url", ""))
        r = runplan(wire(mode="cold"), t / "w2")
        sd = t / "w2" / "session-1-A"
        cold = json.loads((sd / "triplelight.cold.json").read_text()) if (sd / "triplelight.cold.json").exists() else {}
        check("real wiring, cold: only the cold run happens, under the plan runner, with the stubbed reset: scored", r.returncode == 0 and not (sd / "triplelight.warm.json").exists() and cold.get("scored") is True and cold.get("runs_per_query") == 2, json.dumps(cold)[:300] + r.stderr[-300:])
        check("real wiring, cold: one reset and one timed sample per requested repeat (2 runs, 2 resets)", len(cold.get("queries", {}).get("q1", {}).get("runs", [])) == 2 and cold["queries"]["q1"].get("cold_hook_ok") is True)
        man = json.loads((t / "w1" / "session-1-A" / "manifest.json").read_text())
        check("real wiring: the manifest records the plan's hash and its actual sequence", man["protocol"]["plan_sha256"] == h1 and man["protocol"]["order"] == "A")
        check("real wiring: the manifest hashes the dataset and store the session served", man["datasets"][0]["sha256"] == hashlib.sha256((tools / "wire.nt").read_bytes()).hexdigest() and man["stores"][0]["name"] == "wire")
        check("real wiring: the machine lock is released afterwards", not (t / "wlock").exists())
        two = [{"label": "A", "binary": str(fake)}, {"label": "B", "binary": str(fake)}]
        r = runplan(wire(arms=two, sequence="BAAB"), t / "w3")
        man3 = json.loads((t / "w3" / "session-1-B" / "manifest.json").read_text()) if (t / "w3" / "session-1-B" / "manifest.json").exists() else {}
        check("real wiring, BAAB: the manifest records BAAB, not the default ABBA", man3.get("protocol", {}).get("order") == "BAAB")
        check("real wiring, ABBA: four sessions run and abba.py --scored compares the arms", all((t / "w3" / f"session-{i}-{l}" / "triplelight.warm.json").exists() for i, l in enumerate("BAAB", 1)) and (t / "w3" / "abba-triplelight.json").exists() and r.returncode == 0, r.stdout[-300:] + r.stderr[-400:])
    print("PLAN SELFTEST", "PASSED" if ok else "FAILED"); sys.exit(0 if ok else 1)


def main():
    p = argparse.ArgumentParser(); sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run"); r.add_argument("plan"); r.add_argument("--out", required=True); r.add_argument("--receipt"); r.add_argument("--dry-run", action="store_true")
    r.set_defaults(f=cmd_run)
    s = sub.add_parser("selftest"); s.set_defaults(f=cmd_selftest)
    a = p.parse_args()
    try: a.f(a)
    except PlanError as e: sys.exit(f"plan error: {e}")


if __name__ == "__main__":
    main()
