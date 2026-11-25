#!/usr/bin/env python3
"""Run manifest: what a benchmark run was made of, written before any timing. A run without a complete manifest and a fresh passing
machine-check receipt is not scored and cannot appear in a comparison (task #130, phase 2).

  manifest.py write --out DIR --receipt machine-check.json --binary PATH --queries DIR --reference ref.json
                    --dataset NAME:PATH[:SHA256] --store NAME:PATH --system NAME:VERSION:CONFIGFILE ...
                    --runs N --timeout S --seed N --order ABBA [--plan FILE] [--drain-rows N] [--cargo-features F] [--profile release]
  manifest.py verify MANIFEST.json            exit 0 if complete and the machine receipt is fresh and passing; else the problems, exit 1
  manifest.py selftest

The machine receipt is bench/eval/machine-check.sh's `machine-check.json`: `passed`, `timestamp` (UTC), `boot_id`, `quick`, `failures`,
and `model`, `cpu`, `cores`, `ram_gb`, `os`, `kernel`. It qualifies a scored run for 24 hours on the same boot; a `--quick` receipt never does.
"""
import argparse, datetime, hashlib, json, os, pathlib, platform, re, subprocess, sys, tempfile, time

SCHEMA = 1
FRESH_HOURS = 24
MACHINE_FIELDS = ("model", "cpu", "cores", "ram_gb", "os", "kernel")


def sha256_file(path, chunk=1 << 22):
    if not os.path.isfile(path): raise SystemExit(f"cannot digest {path}: not a file")
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while block := f.read(chunk): h.update(block)
    return h.hexdigest()


def sha256_tree(path):
    """One digest over a directory: every file's relative path, size and content digest, in sorted order."""
    h = hashlib.sha256()
    root = pathlib.Path(path)
    files = sorted(x for x in root.rglob("*") if x.is_file()) if root.is_dir() else []
    if not files: raise SystemExit(f"cannot digest {path}: not a directory with files")      # the empty-tree digest must never pass for a real input
    for p in files:
        h.update(f"{p.relative_to(root)}\0{p.stat().st_size}\0{sha256_file(p)}\n".encode())
    return h.hexdigest()


def run_text(cmd, cwd=None):
    try: return subprocess.run(cmd, capture_output=True, text=True, cwd=cwd, timeout=60).stdout.strip()
    except Exception: return ""


def boot_id():
    """Identifies this boot: a reboot can reset the frequency governor and turbo settings, so a receipt from another boot does not count."""
    p = pathlib.Path("/proc/sys/kernel/random/boot_id")
    if p.exists(): return p.read_text().strip()
    return boottime_seconds(run_text(["sysctl", "-n", "kern.boottime"]))      # macOS


def boottime_seconds(text):
    """The `sec` of macOS `kern.boottime` (`{ sec = 1700000000, usec = 123456 } Tue ...`); `usec` differs between reads of one boot, so only seconds are compared."""
    m = re.search(r"\bsec = (\d+)", text)
    return m.group(1) if m else text


def parse_time(text):
    return datetime.datetime.fromisoformat(text.replace("Z", "+00:00"))


def check_receipt(receipt, now=None, current_boot=None):
    """Problems with a machine-check receipt (an empty list means it qualifies a scored run)."""
    problems = []
    if not isinstance(receipt, dict): return ["machine receipt is not a JSON object"]
    if receipt.get("passed") is not True: problems.append("machine check did not pass" + (f": {receipt.get('failures')}" if receipt.get("failures") else ""))
    if receipt.get("quick") is not False: problems.append("a quick machine check never qualifies a scored run")
    for f in MACHINE_FIELDS:
        if receipt.get(f) in (None, "", 0): problems.append(f"machine receipt lacks {f}")
    try:
        age = (now or datetime.datetime.now(datetime.timezone.utc)) - parse_time(receipt["timestamp"])
        if age > datetime.timedelta(hours=FRESH_HOURS): problems.append(f"machine receipt is {age.total_seconds() / 3600:.1f} h old (limit {FRESH_HOURS} h)")
        if age < datetime.timedelta(minutes=-5): problems.append("machine receipt is dated in the future")
    except Exception: problems.append("machine receipt has no valid timestamp")
    cur = current_boot if current_boot is not None else boot_id()
    if not receipt.get("boot_id") or receipt.get("boot_id") != cur: problems.append("machine receipt is from another boot (governor and turbo settings may have changed)")
    return problems


REQUIRED = {
    "git": ("commit", "dirty"), "build": ("profile", "rustc", "binary_sha256"),
    "protocol": ("order", "runs", "timeout_s", "seed", "mode", "drain_rows", "expect_unsupported", "persisted_store"),
    "machine": ("receipt_sha256", "receipt_passed_at", *MACHINE_FIELDS),
}
HEX = re.compile(r"^[0-9a-f]{64}$"); COMMIT = re.compile(r"^[0-9a-f]{40}$")


def is_int(x, lo=0): return isinstance(x, int) and not isinstance(x, bool) and x >= lo


def verify(m, receipt=None, now=None, current_boot=None):
    """Problems with a manifest: missing sections and fields, wrong types or digest shapes, empty datasets or systems, a persisted-store
    run without a store digest, machine facts that disagree with the receipt, and (when the receipt is given) an unqualified machine."""
    p = []
    def need(cond, msg):
        if not cond: p.append(msg)
    need(m.get("schema") == SCHEMA, f"schema is {m.get('schema')}, expected {SCHEMA}")
    for section, fields in REQUIRED.items():
        s = m.get(section)
        if not isinstance(s, dict): p.append(f"missing section {section}"); continue
        for f in fields:
            if f not in s or s[f] is None or s[f] == "": p.append(f"{section}.{f} is missing")
    need(isinstance(m.get("run_id"), str) and m.get("run_id"), "run_id is missing")
    g, b, pr, mc = (m.get(k) if isinstance(m.get(k), dict) else {} for k in ("git", "build", "protocol", "machine"))
    need(isinstance(g.get("commit"), str) and COMMIT.match(g.get("commit", "")), "git.commit is not a 40-hex commit")
    need(isinstance(g.get("dirty"), bool), "git.dirty is not a boolean")
    need(g.get("dirty") is not True, "git.dirty: the build tree had uncommitted changes")
    need(isinstance(b.get("binary_sha256"), str) and HEX.match(b.get("binary_sha256", "")), "build.binary_sha256 is not a sha256 digest")
    for key in ("queries_sha256", "reference_sha256"):
        need(isinstance(m.get(key), str) and HEX.match(m.get(key, "")), f"{key} is not a sha256 digest")
    need(is_int(pr.get("runs"), 1), "protocol.runs is not a positive integer")
    need(isinstance(pr.get("timeout_s"), (int, float)) and not isinstance(pr.get("timeout_s"), bool) and pr.get("timeout_s", 0) > 0, "protocol.timeout_s is not positive")
    need(is_int(pr.get("seed"), -(1 << 62)), "protocol.seed is not an integer")
    need(is_int(pr.get("drain_rows"), 0), "protocol.drain_rows is not a non-negative integer")
    need(pr.get("mode") in ("warm", "cold"), "protocol.mode is not warm or cold")
    eu = pr.get("expect_unsupported")
    need(isinstance(eu, dict) and all(isinstance(v, list) for v in eu.values()), "protocol.expect_unsupported is not a map from system to a list of case IDs")
    need(isinstance(pr.get("persisted_store"), bool), "protocol.persisted_store is not a boolean")
    if pr.get("mode") == "cold":
        need(isinstance(pr.get("cold_hook"), str) and cold_hook_parts(pr.get("cold_hook", "")) is not None,
             "a cold run needs protocol.cold_hook to be `restart.sh --cold` (the reset that drops the page cache and fails when it cannot)")
        need(isinstance(pr.get("cold_hook_sha256"), str) and HEX.match(pr.get("cold_hook_sha256", "")), "a cold run needs protocol.cold_hook_sha256, the digest of the reset procedure")
    need(isinstance(mc.get("receipt_sha256"), str) and HEX.match(mc.get("receipt_sha256", "")), "machine.receipt_sha256 is not a sha256 digest")
    for key, fields in (("datasets", ("name", "bytes", "sha256")), ("systems", ("name", "version", "config_sha256", "url"))):
        items = m.get(key)
        if not items: p.append(f"{key} is empty"); continue
        for it in items:
            for f in fields:
                if it.get(f) in (None, ""): p.append(f"{key}[{it.get('name', '?')}].{f} is missing")
            digest = it.get("sha256") if key == "datasets" else it.get("config_sha256")
            need(isinstance(digest, str) and HEX.match(digest or ""), f"{key}[{it.get('name', '?')}] digest is not a sha256 digest")
            if key == "systems": need(isinstance(it.get("clear_cache_url"), str), f"systems[{it.get('name', '?')}].clear_cache_url is missing (use an empty string for none)")
            if key == "datasets":
                need(is_int(it.get("bytes"), 1), f"datasets[{it.get('name', '?')}].bytes is not positive")
                need(it.get("sha256_verified") is True, f"datasets[{it.get('name', '?')}].sha256 was supplied, not verified against the data")
    if pr.get("persisted_store") is True:
        stores = m.get("stores")
        need(isinstance(stores, list) and stores and all(isinstance(x, dict) and HEX.match(str(x.get("tree_sha256", ""))) for x in stores),
             "a run on a persisted store needs a store tree digest (stores[].tree_sha256)")
    if receipt is not None:
        p += check_receipt(receipt, now, current_boot)
        for f in MACHINE_FIELDS:
            need(mc.get(f) == receipt.get(f), f"machine.{f} disagrees with the machine receipt")
        need(mc.get("receipt_passed_at") == receipt.get("timestamp"), "machine.receipt_passed_at disagrees with the machine receipt")
    return p


def run_bindings(m, *, queries_sha256, reference_sha256, system, url, clear_cache_url, runs, timeout_s, seed, drain_rows, mode, expect_unsupported, cold_hook=""):
    """Problems where the run about to start differs from the one the manifest describes: the query set, the reference, the engine, the
    protocol, the endpoint and the cache-clear policy, and the mode and expected-unsupported policy must all be what was recorded."""
    p, pr = [], m.get("protocol", {})
    def same(label, got, want):
        if got != want: p.append(f"{label} differs from the manifest (run {got!r}, manifest {want!r})")
    same("query set digest", queries_sha256, m.get("queries_sha256")); same("reference digest", reference_sha256, m.get("reference_sha256"))
    listed = {x.get("name"): x for x in m.get("systems", [])}
    if system not in listed: p.append(f"system {system!r} is not listed in the manifest")
    else:
        same("endpoint url", url, listed[system].get("url"))
        same("result-cache clear url", clear_cache_url or "", listed[system].get("clear_cache_url"))
    same("runs", runs, pr.get("runs")); same("timeout", timeout_s, pr.get("timeout_s")); same("seed", seed, pr.get("seed"))
    same("drain threshold", drain_rows, pr.get("drain_rows")); same("mode", mode, pr.get("mode"))
    declared = pr.get("expect_unsupported", {})
    same("expected-unsupported cases for this system", sorted(expect_unsupported), sorted(declared.get(system, []) if isinstance(declared, dict) else []))
    if mode == "cold":
        same("cold-start hook", cold_hook, pr.get("cold_hook"))
        same("reset procedure digest (restart.sh and lib.sh as they are now)", cold_hook_digest(cold_hook), pr.get("cold_hook_sha256"))
    return p


RESET_DIR = pathlib.Path(__file__).resolve().parent      # bench/eval of this checkout: the only place the reset procedure may come from


def cold_hook_parts(command):
    """The cold-start hook command as `[restart.sh, --cold]`, or None. Exactly this repository's bench/eval/restart.sh with exactly one
    argument, `--cold` (the reset that stops the system, drops the page cache and fails when it cannot): a script of the same name
    elsewhere, a different argument list, or `--cold` in any other position is refused, because restart.sh reads only its first argument."""
    import shlex
    try: tokens = shlex.split(command)
    except ValueError: return None
    if len(tokens) != 2 or tokens[1] != "--cold": return None
    if not os.path.isfile(tokens[0]) or os.path.realpath(tokens[0]) != os.path.realpath(RESET_DIR / "restart.sh"): return None
    return tokens


def cold_hook_digest(command):
    """sha256 over restart.sh and the lib.sh it sources: the reset procedure that will run, so editing either changes the digest."""
    parts = cold_hook_parts(command)
    if not parts: return ""
    h = hashlib.sha256()
    for f in (RESET_DIR / "restart.sh", RESET_DIR / "lib.sh"):
        if not f.is_file(): return ""
        h.update(f.read_bytes())
    return h.hexdigest()


def split(spec, n, what):
    parts = spec.split(":", n - 1)
    if len(parts) < n - 1 or not parts[0]: raise SystemExit(f"{what} must look like {':'.join(['NAME'] + ['…'] * (n - 1))}, got {spec!r}")
    return parts


def cmd_write(a):
    receipt = json.load(open(a.receipt))
    problems = check_receipt(receipt)
    if problems and not a.allow_unqualified: sys.exit("machine receipt does not qualify a scored run:\n  " + "\n  ".join(problems))
    root = pathlib.Path(os.environ.get("MANIFEST_REPO_ROOT") or pathlib.Path(__file__).resolve().parents[2])      # the override is for the self-tests
    expect = {}
    for spec in a.expect_unsupported:
        system, _, cases = spec.partition("=")
        if not system or not _: sys.exit(f"--expect-unsupported must look like SYSTEM=case,case, got {spec!r}")
        expect[system] = sorted(x for x in re.split(r"[,\s]+", cases.strip()) if x)
    git = lambda *args: run_text(["git", *args], cwd=root)
    datasets = []
    for spec in a.dataset:
        name, path, *sha = split(spec, 3, "--dataset")
        if not os.path.isfile(path): sys.exit(f"dataset {name}: {path} is not a file")
        actual = sha256_file(path)
        if sha and sha[0] and sha[0] != actual and not a.trust_supplied_digests:
            sys.exit(f"dataset {name}: the supplied digest {sha[0]} does not match the data ({actual})")
        supplied_unchecked = bool(sha and sha[0]) and a.trust_supplied_digests
        datasets.append({"name": name, "bytes": os.path.getsize(path), "sha256": sha[0] if supplied_unchecked else actual, "sha256_verified": not supplied_unchecked})
    systems = []
    for spec in a.system:
        name, version, config = split(spec, 3, "--system")
        urls, clears = dict(x.split("=", 1) for x in a.system_url), dict(x.split("=", 1) for x in a.system_clear_cache)
        if name not in urls: sys.exit(f"system {name}: give --system-url {name}=URL, the endpoint the run will query")
        systems.append({"name": name, "version": version, "config_sha256": sha256_file(config), "url": urls[name], "clear_cache_url": clears.get(name, ""),
                        **({"memory_budget": a.memory_budget} if a.memory_budget else {})})
    stores = []
    for spec in a.store:
        name, path = split(spec, 2, "--store")
        stores.append({"name": name, "tree_sha256": sha256_tree(path)})
    m = {
        "schema": SCHEMA, "run_id": a.run_id or f"{datetime.datetime.now(datetime.timezone.utc):%Y%m%dT%H%M%SZ}",
        "started_utc": f"{datetime.datetime.now(datetime.timezone.utc):%Y-%m-%dT%H:%M:%SZ}",
        "git": {"commit": git("rev-parse", "HEAD"), "dirty": bool(git("status", "--porcelain", "--untracked-files=no"))},
        "driver_commit": git("log", "-1", "--format=%H", "--", "bench/eval"),
        "build": {"profile": a.profile, "cargo_features": a.cargo_features, "rustc": run_text(["rustc", "--version"]),
                  "rustflags": os.environ.get("RUSTFLAGS", ""), "binary_sha256": sha256_file(a.binary)},
        "datasets": datasets, "stores": stores,
        "queries_sha256": sha256_tree(a.queries), "reference_sha256": sha256_file(a.reference),
        "systems": systems,
        "protocol": {"plan_sha256": sha256_file(a.plan) if a.plan else "", "order": a.order, "runs": a.runs, "timeout_s": a.timeout,
                     "seed": a.seed, "drain_rows": a.drain_rows, "mode": a.mode, "persisted_store": not a.no_store,
                     **({"cold_hook": a.cold_hook, "cold_hook_sha256": cold_hook_digest(a.cold_hook)} if a.mode == "cold" else {}),
                     "expect_unsupported": expect},
        "machine": {"receipt_sha256": sha256_file(a.receipt), "receipt_passed_at": receipt.get("timestamp", ""),
                    **{f: receipt.get(f) for f in MACHINE_FIELDS}},
        "python": platform.python_version(),
    }
    if not a.store and not a.no_store: sys.exit("give --store NAME:PATH for every persisted store the run reads, or --no-store if the systems keep no store")
    out = pathlib.Path(a.out); out.mkdir(parents=True, exist_ok=True)
    problems = verify(m, receipt)
    if problems and not a.allow_unqualified: sys.exit("manifest incomplete:\n  " + "\n  ".join(problems))
    path = out / "manifest.json"
    path.write_text(json.dumps(m, indent=1, sort_keys=True) + "\n")
    import shutil
    shutil.copyfile(a.receipt, out / "machine-check.json")      # byte for byte: the manifest holds its digest
    print(path)


def load_checked(manifest_path, receipt_path=None):
    """The driver's entry: the manifest dict and its problems, with the machine receipt stored beside it (or given)."""
    mp = pathlib.Path(manifest_path)
    m = json.loads(mp.read_text())
    rp = pathlib.Path(receipt_path) if receipt_path else mp.parent / "machine-check.json"
    problems = []
    try: receipt = json.loads(rp.read_text())
    except Exception as e: return m, [f"cannot read the machine receipt {rp.name}: {e}"]
    if sha256_file(rp) != m.get("machine", {}).get("receipt_sha256"):
        problems.append("machine receipt does not match the digest in the manifest")
    return m, problems + verify(m, receipt)


def cmd_verify(a):
    m, problems = load_checked(a.manifest, a.receipt)
    if problems: sys.exit("manifest does not qualify a scored run:\n  " + "\n  ".join(problems))
    print("manifest ok:", m["run_id"])


def good_receipt(now, boot):
    return {"passed": True, "quick": False, "boot_id": boot, "timestamp": f"{now:%Y-%m-%dT%H:%M:%SZ}", "failures": [], **{f: "x" for f in MACHINE_FIELDS}}


def good_manifest(receipt=None):
    r = receipt or good_receipt(datetime.datetime.now(datetime.timezone.utc), boot_id())
    return {
        "schema": SCHEMA, "run_id": "t", "git": {"commit": "a" * 40, "dirty": False},
        "build": {"profile": "release", "rustc": "rustc 1", "binary_sha256": "b" * 64},
        "datasets": [{"name": "lubm1", "bytes": 10, "sha256": "c" * 64, "sha256_verified": True}], "queries_sha256": "d" * 64, "reference_sha256": "e" * 64,
        "stores": [{"name": "lubm1", "tree_sha256": "1" * 64}],
        "systems": [{"name": "triplelight", "version": "x", "config_sha256": "f" * 64, "url": "http://127.0.0.1:7878/sparql", "clear_cache_url": ""}],
        "protocol": {"order": "ABBA", "runs": 5, "timeout_s": 300, "seed": 0, "drain_rows": 200000, "mode": "warm", "expect_unsupported": {}, "persisted_store": True},
        "machine": {"receipt_sha256": "0" * 64, "receipt_passed_at": r["timestamp"], **{f: r[f] for f in MACHINE_FIELDS}},
    }


def cmd_selftest(a):
    ok = True
    now = datetime.datetime.now(datetime.timezone.utc)
    def check(label, problems, expect_substr=None):
        nonlocal ok
        good = (not problems) if expect_substr is None else (bool(problems) and any(expect_substr in x for x in problems))
        print(f"manifest {label:<64} {'ok' if good else 'FAIL ' + str(problems)}"); ok &= good
    r = good_receipt(now, "boot-1"); m = good_manifest(r)
    check("a complete manifest with a fresh passing receipt", verify(m, r, now, "boot-1"))
    for section in ("git", "build", "protocol", "machine"):
        bad = {k: v for k, v in m.items() if k != section}
        check(f"a manifest without {section} is refused", verify(bad, r, now, "boot-1"), f"missing section {section}")
    bad = json.loads(json.dumps(m)); bad["build"]["binary_sha256"] = ""
    check("an empty binary digest is refused", verify(bad, r, now, "boot-1"), "build.binary_sha256")
    bad = json.loads(json.dumps(m)); bad["datasets"] = []
    check("no datasets is refused", verify(bad, r, now, "boot-1"), "datasets is empty")
    bad = json.loads(json.dumps(m)); bad["systems"][0]["config_sha256"] = ""
    check("a system without a pinned config digest is refused", verify(bad, r, now, "boot-1"), "config_sha256")
    bad = json.loads(json.dumps(m)); bad["git"]["dirty"] = True
    check("a dirty build tree is refused", verify(bad, r, now, "boot-1"), "git.dirty")
    check("a failed machine check is refused", verify(m, {**r, "passed": False, "failures": [{"check": "idle", "why": "python3 at 50%"}]}, now, "boot-1"), "did not pass")
    check("a quick machine check is refused", verify(m, {**r, "quick": True}, now, "boot-1"), "quick")
    check("a receipt from another boot is refused", verify(m, r, now, "boot-2"), "another boot")
    old = good_receipt(now - datetime.timedelta(hours=25), "boot-1")
    check("a 25-hour-old receipt is refused", verify(good_manifest(old), old, now, "boot-1"), "old")
    almost = good_receipt(now - datetime.timedelta(hours=23), "boot-1")
    check("a 23-hour-old receipt still qualifies", verify(good_manifest(almost), almost, now, "boot-1"))
    check("a receipt without a timestamp is refused", verify(m, {k: v for k, v in r.items() if k != "timestamp"}, now, "boot-1"), "timestamp")
    check("a receipt without a machine model is refused", verify(m, {**r, "model": ""}, now, "boot-1"), "lacks model")
    def edit(f):
        x = json.loads(json.dumps(m)); f(x); return x
    check("no run id is refused", verify(edit(lambda x: x.pop("run_id")), r, now, "boot-1"), "run_id")
    check("zero repeats are refused", verify(edit(lambda x: x["protocol"].update(runs=0)), r, now, "boot-1"), "protocol.runs")
    check("a non-digest binary hash is refused", verify(edit(lambda x: x["build"].update(binary_sha256="nothex")), r, now, "boot-1"), "binary_sha256")
    check("a non-boolean dirty flag is refused", verify(edit(lambda x: x["git"].update(dirty="no")), r, now, "boot-1"), "not a boolean")
    check("CPU metadata that disagrees with the receipt is refused", verify(edit(lambda x: x["machine"].update(cpu="other")), r, now, "boot-1"), "machine.cpu disagrees")
    check("a persisted-store run without a store digest is refused", verify(edit(lambda x: x.update(stores=[])), r, now, "boot-1"), "store tree digest")
    check("a store-less run needs no store digest", verify(edit(lambda x: (x.update(stores=[]), x["protocol"].update(persisted_store=False))), r, now, "boot-1"))
    check("a system without an endpoint url is refused", verify(edit(lambda x: x["systems"][0].pop("url")), r, now, "boot-1"), "url is missing")
    check("an unverified supplied dataset digest is refused", verify(edit(lambda x: x["datasets"][0].update(sha256_verified=False)), r, now, "boot-1"), "not verified")
    cm = edit(lambda x: x["protocol"].update(mode="cold"))
    check("a cold manifest without a hook is refused", verify(cm, r, now, "boot-1"), "cold_hook")
    check("a cold manifest whose hook is `true` is refused", verify(edit(lambda x: x["protocol"].update(mode="cold", cold_hook="true", cold_hook_sha256="2" * 64)), r, now, "boot-1"), "restart.sh --cold")
    real = f"'{RESET_DIR / 'restart.sh'}' --cold"
    check("a cold manifest with this repo's restart.sh --cold and a digest passes", verify(edit(lambda x: x["protocol"].update(mode="cold", cold_hook=real, cold_hook_sha256="2" * 64)), r, now, "boot-1"))
    for label, command in (("restart.sh without --cold", f"'{RESET_DIR / 'restart.sh'}'"), ("--cold in the wrong position", f"'{RESET_DIR / 'restart.sh'}' x --cold"),
                           ("--cold first, then more arguments", f"'{RESET_DIR / 'restart.sh'}' --cold x"), ("a restart.sh from another directory", "/tmp/restart.sh --cold"),
                           ("another script with the same name copied elsewhere", f"'{pathlib.Path(tempfile.gettempdir()) / 'restart.sh'}' --cold")):
        check(f"{label} is not the cold reset", [] if cold_hook_parts(command) is None else ["accepted"])
    bind = dict(queries_sha256="d" * 64, reference_sha256="e" * 64, system="triplelight", url="http://127.0.0.1:7878/sparql", clear_cache_url="", runs=5, timeout_s=300, seed=0, drain_rows=200000, mode="warm", expect_unsupported=[])
    check("a run that matches its manifest binds", run_bindings(m, **bind))
    for field, other, word in (("queries_sha256", "9" * 64, "query set"), ("reference_sha256", "9" * 64, "reference"), ("system", "virtuoso", "system"), ("url", "http://127.0.0.1:9999/sparql", "endpoint"), ("clear_cache_url", "http://127.0.0.1:1/clear", "cache-clear"), ("runs", 3, "runs"),
                               ("timeout_s", 60, "timeout"), ("seed", 7, "seed"), ("drain_rows", 1, "drain"), ("mode", "cold", "mode"), ("expect_unsupported", ["q3"], "expected-unsupported")):
        check(f"a different {word} than the manifest's is refused", run_bindings(m, **{**bind, field: other}), "")
    check("macOS boot time keeps only the seconds", [] if boottime_seconds("{ sec = 1700000000, usec = 123456 } Tue Nov 14 22:13:20 2023") == "1700000000" else ["wrong"])
    # write then verify, end to end, in a temporary directory
    with tempfile.TemporaryDirectory() as t:
        t = pathlib.Path(t)
        (t / "bin").write_bytes(b"binary"); (t / "q").mkdir(); (t / "q" / "q1.rq").write_text("SELECT * WHERE {}")
        (t / "ref.json").write_text("{}"); (t / "data.nt").write_text("<a> <b> <c> .\n"); (t / "cfg").write_text("memory=1")
        (t / "store").mkdir(); (t / "store" / "f").write_text("x")
        rec = good_receipt(datetime.datetime.now(datetime.timezone.utc), boot_id()); (t / "rec.json").write_text(json.dumps(rec))
        base = [sys.executable, __file__, "write", "--out", str(t / "out"), "--receipt", str(t / "rec.json"), "--binary", str(t / "bin"), "--queries", str(t / "q"),
                "--reference", str(t / "ref.json"), "--dataset", f"tiny:{t / 'data.nt'}", "--store", f"tiny:{t / 'store'}", "--system", f"ts:1.0:{t / 'cfg'}", "--system-url", "ts=http://127.0.0.1:1/sparql"]
        bad = subprocess.run(base[:-1] + [f"ts:1.0:{t / 'cfg'}", "--dataset", f"x:{t / 'data.nt'}:{'0' * 64}"], capture_output=True, text=True)
        print(f"manifest {'a supplied dataset digest that does not match the data is refused':<64} {'ok' if bad.returncode != 0 and 'does not match' in bad.stderr else 'FAIL'}"); ok &= bad.returncode != 0 and "does not match" in bad.stderr
        gone = subprocess.run([x if x != str(t / 'q') else str(t / 'nope') for x in base], capture_output=True, text=True)
        print(f"manifest {'a missing query directory is refused, not digested as empty':<64} {'ok' if gone.returncode != 0 and 'cannot digest' in gone.stderr else 'FAIL'}"); ok &= gone.returncode != 0 and "cannot digest" in gone.stderr
        w = subprocess.run(base, capture_output=True, text=True)
        wrote = w.returncode == 0 or "git.dirty" in w.stderr      # a checkout with local edits is refused by design; the write path is still exercised
        print(f"manifest {'write runs end to end (or is refused only for a dirty tree)':<64} {'ok' if wrote else 'FAIL ' + w.stderr[-300:]}"); ok &= wrote
        if w.returncode == 0:
            v = subprocess.run([sys.executable, __file__, "verify", str(t / "out" / "manifest.json")], capture_output=True, text=True)
            print(f"manifest {'verify accepts what write produced':<64} {'ok' if v.returncode == 0 else 'FAIL ' + v.stderr[-300:]}"); ok &= v.returncode == 0
            mm = json.loads((t / "out" / "manifest.json").read_text()); mm["build"]["binary_sha256"] = ""; (t / "out" / "manifest.json").write_text(json.dumps(mm))
            v = subprocess.run([sys.executable, __file__, "verify", str(t / "out" / "manifest.json")], capture_output=True, text=True)
            print(f"manifest {'verify refuses a manifest edited to be incomplete':<64} {'ok' if v.returncode != 0 else 'FAIL'}"); ok &= v.returncode != 0
    print("MANIFEST SELFTEST", "PASSED" if ok else "FAILED"); sys.exit(0 if ok else 1)


def main():
    p = argparse.ArgumentParser(); sub = p.add_subparsers(dest="cmd", required=True)
    w = sub.add_parser("write"); w.add_argument("--out", required=True); w.add_argument("--receipt", required=True); w.add_argument("--binary", required=True)
    w.add_argument("--queries", required=True); w.add_argument("--reference", required=True)
    w.add_argument("--dataset", action="append", default=[]); w.add_argument("--store", action="append", default=[]); w.add_argument("--system", action="append", default=[])
    w.add_argument("--plan"); w.add_argument("--run-id"); w.add_argument("--profile", default="release"); w.add_argument("--cargo-features", default="")
    w.add_argument("--order", default="ABBA"); w.add_argument("--runs", type=int, default=5); w.add_argument("--timeout", type=float, default=300)
    w.add_argument("--seed", type=int, default=0); w.add_argument("--drain-rows", type=int, default=200000); w.add_argument("--memory-budget", default="")
    w.add_argument("--system-url", action="append", default=[], help="NAME=URL: the endpoint the run queries for that system (bound: a different --url is not scored)")
    w.add_argument("--system-clear-cache", action="append", default=[], help="NAME=URL: the result-cache clear URL used before every request; omit for systems without one (bound the same way)")
    w.add_argument("--cold-hook", default="", help="for --mode cold: the reset command, which must be `bench/eval/restart.sh --cold`; its digest is recorded and checked at run time")
    w.add_argument("--mode", choices=["warm", "cold"], default="warm"); w.add_argument("--expect-unsupported", action="append", default=[], help="SYSTEM=case,case: cases that system's configuration is known not to answer (repeat per system)")
    w.add_argument("--no-store", action="store_true", help="the systems in this run keep no persisted store (otherwise every store needs --store)")
    w.add_argument("--trust-supplied-digests", action="store_true", help="accept a dataset digest given as NAME:PATH:SHA256 without hashing the data; the manifest records it as unverified, so the run is not scored")
    w.add_argument("--allow-unqualified", action="store_true", help="write the manifest even when the machine receipt or a field fails; the run is then not scored (exploration)")
    w.set_defaults(f=cmd_write)
    v = sub.add_parser("verify"); v.add_argument("manifest"); v.add_argument("--receipt"); v.set_defaults(f=cmd_verify)
    s = sub.add_parser("selftest"); s.set_defaults(f=cmd_selftest)
    a = p.parse_args(); a.f(a)


if __name__ == "__main__":
    main()
