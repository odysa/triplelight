#!/usr/bin/env python3
"""Evaluation driver (docs/eval-plan/PLAN.md, section "Server suites ... and the driver").

Warm means: buffers, the OS page cache and indexes are warm, but NO system may answer from a result cache. A server that has one
(QLever) is emptied before every request, warm-up included (`--clear-cache-url`); if that call fails the run stops and the entry
records `cache_clear_failed`. TripleLight has no result cache (block caches and a per-snapshot seek memo only); the other
servers document none, which the rental re-checks (docs/qlever-cache-check-v1).

Large answers (reference rows >= --drain-rows): the warm-up run is the one hashed check; the timed runs only drain the body (time to
last byte) and must match its byte count (docs/large-answer-timing-v1).

  driver.py reference --queries DIR --url URL --out ref.json     answers of the reference system (Jena), untimed
  driver.py run --system NAME --url URL --queries DIR --reference ref.json --out out.json [--mode warm|cold]
  driver.py selftest                                             classifier and streaming checks on fake endpoints

One system at a time. Every response is hashed while it streams (rows are not kept), so a 7.9M-row answer needs bounded
memory. Each query and system gets a record with a class: pass, value-compatible, mismatch, incomplete, timeout, error.
Standard library only."""
import argparse, codecs, datetime, hashlib, pathlib, http.client, json, os, platform, random, re, statistics, subprocess, sys, time
import urllib.parse, urllib.request

XSD = "http://www.w3.org/2001/XMLSchema#"
# Datatypes that some systems report as xsd:integer and others as a narrower integer type (QLever returns xsd:int).
INTEGER_LIKE = {XSD + t for t in ("int", "long", "short", "byte", "nonNegativeInteger", "positiveInteger", "negativeInteger",
                                  "nonPositiveInteger", "unsignedInt", "unsignedLong", "unsignedShort", "unsignedByte")}
CAP = 1048576   # Virtuoso's HTTP endpoint stops here whatever ResultSetMaxRows says
CHUNK = 1 << 20


HEAD_BYTES = 1 << 20    # how much of a drained body is kept to read its projected variables
KEEP_ROWS = False      # set per request by one_run(keep=True): the case policy needs the typed answer
KEEP_CAP = 100_000     # answers with more rows are never handed to a policy (they stay hash-compared)
MIN_READ = 64 << 10      # stream_fold gathers chunks up to this size per read; the self-test sets 0 to exercise awkward chunk boundaries
class Timeout(Exception): pass
class Truncated(Exception): pass


def term(b, normalize):
    """Canonical text of one binding, as in bench/run.py; `normalize` maps narrower integer types to xsd:integer."""
    if b is None: return "-"
    kind = b["type"]
    v = b["value"] if kind != "bnode" else "_"                      # blank labels differ across systems
    kind = "literal" if kind == "typed-literal" else kind           # Virtuoso's SPARQL-JSON 2008 spelling
    dt = b.get("datatype", "")
    if dt == XSD + "string": dt = ""                                # RDF 1.1: a plain literal is an xsd:string
    if normalize and dt in INTEGER_LIKE: dt = XSD + "integer"
    return f'{kind}|{v}|{dt}|{b.get("xml:lang", "").lower()}'


class Fold:
    """Order-independent bag hash (sum of 128-bit digests), an order-sensitive hash, and a row count; two variants,
    exact and with narrower integer types normalized."""
    def __init__(self, vars_):
        self.vars, self.n, self.bag, self.bag_norm = vars_, 0, 0, 0
        self.seq = hashlib.sha1()
        self.rows = [] if KEEP_ROWS else None      # the full typed answer, only for a case policy and only up to KEEP_CAP rows
    def add(self, row):
        if self.rows is not None:
            if len(self.rows) < KEEP_CAP: self.rows.append(row)
            else: self.rows = None; self.too_big = True
        a = "\t".join(term(row.get(v), False) for v in self.vars)
        b = "\t".join(term(row.get(v), True) for v in self.vars)
        self.n += 1
        self.bag = (self.bag + int.from_bytes(hashlib.blake2b(a.encode(), digest_size=16).digest(), "big")) % (1 << 128)
        self.bag_norm = (self.bag_norm + int.from_bytes(hashlib.blake2b(b.encode(), digest_size=16).digest(), "big")) % (1 << 128)
        self.seq.update(a.encode() + b"\n")
    def result(self):
        r = {"rows": self.n, "bag": f"{self.bag:032x}", "bag_norm": f"{self.bag_norm:032x}", "seq": self.seq.hexdigest(), "vars": list(self.vars)}
        if self.rows is not None: r["answer"] = {"vars": self.vars, "rows": self.rows}
        return r


def fold_boolean(value):
    f = Fold([]); f.n = 1
    h = int.from_bytes(hashlib.blake2b(str(bool(value)).encode(), digest_size=16).digest(), "big")
    f.bag = f.bag_norm = h; f.seq.update(str(bool(value)).encode())
    return f.result()


def head_vars(text):
    """The projected variables from the top-level `head` object of a SPARQL JSON document, or None when they cannot be read from `text` (the first
    bytes of the body). Walks the top-level members, skipping any other member (metadata before the head, say) by bracket matching that is aware of
    strings, so a `vars` list inside some other object is never mistaken for the head's. Shared by the folding and the draining paths."""
    dec = json.JSONDecoder()
    i, n = 0, len(text)
    def skip_ws(i):
        while i < n and text[i] in " \t\r\n": i += 1
        return i
    def skip_value(i):
        """Index just past the JSON value starting at i, or None if it does not end inside `text`."""
        i = skip_ws(i)
        if i >= n: return None
        if text[i] in "{[":
            depth, in_str, esc = 0, False, False
            while i < n:
                c = text[i]
                if in_str:
                    if esc: esc = False
                    elif c == "\\": esc = True
                    elif c == '"': in_str = False
                elif c == '"': in_str = True
                elif c in "{[": depth += 1
                elif c in "}]":
                    depth -= 1
                    if depth == 0: return i + 1
                i += 1
            return None
        try: return dec.raw_decode(text, i)[1]
        except ValueError: return None
    i = skip_ws(i)
    if i >= n or text[i] != "{": return None
    i += 1
    while True:
        i = skip_ws(i)
        if i >= n or text[i] == "}": return None
        if text[i] == ",": i += 1; continue
        try: key, i = dec.raw_decode(text, i)
        except ValueError: return None
        i = skip_ws(i)
        if i >= n or text[i] != ":": return None
        i = skip_ws(i + 1)
        if key == "head":
            try: value, _ = dec.raw_decode(text, i)
            except ValueError: return None
            v = value.get("vars") if isinstance(value, dict) else None
            return v if isinstance(v, list) else None
        i = skip_value(i)
        if i is None: return None


def stream_fold(chunks, deadline):
    """Fold a SPARQL JSON result read from `chunks` (an iterator of bytes) without keeping the rows.
    Raises Truncated if the document ends inside the bindings array, Timeout past `deadline`."""
    dec = json.JSONDecoder()
    text = codecs.getincrementaldecoder("utf-8")()
    buf, pos, ended = "", 0, False

    def more():
        nonlocal buf, pos, ended
        if time.monotonic() > deadline: raise Timeout()
        # gather small chunks into one read of at least 64 KB: `buf += ...` copies the buffer, so tiny chunks (the self-test
        # feeds one binding at a time) made the run slow and fragmented the heap (565 MB peak at 5M rows, flat from 100k rows up)
        parts, size = [], 0
        try:
            while True:
                c = next(chunks); parts.append(c); size += len(c)
                if size >= MIN_READ: break
        except StopIteration:
            ended = True
            if not parts: return False
        if pos > (1 << 20): buf, pos = buf[pos:], 0
        buf += text.decode(b"".join(parts))
        return True

    # head: read until the bindings array (or a boolean) starts
    while True:
        m = re.search(r'"bindings"\s*:\s*\[', buf)
        if m: break
        if '"boolean"' in buf and not re.search(r'"bindings"', buf):
            while more(): pass          # an ASK answer is tiny: read it all
            return fold_boolean(json.loads(buf)["boolean"])
        if not more():
            raise Truncated("document ended before the bindings array")
    head = buf[:m.start()]
    vm = head_vars(buf[:m.start()] + "}")      # everything before the bindings array, closed: the top-level `head` object lies in it
    if vm is None:
        # results before head: read the whole document and fold it the plain way (small answers only)
        while more(): pass
        doc = json.loads(buf); f = Fold(doc["head"]["vars"])
        for row in doc["results"]["bindings"]: f.add(row)
        return f.result()
    fold = Fold(vm)
    pos = m.end()
    while True:
        while True:
            while pos < len(buf) and buf[pos] in " \t\r\n,": pos += 1
            if pos < len(buf): break
            if not more(): raise Truncated("document ended inside the bindings array")
        if buf[pos] == "]": return fold.result()
        try:
            obj, end = dec.raw_decode(buf, pos)
        except ValueError:
            if not more(): raise Truncated("document ended inside a binding")
            continue
        fold.add(obj); pos = end


def post(url, query, timeout):
    """POST the query; returns (status, chunk iterator, connection). The iterator yields the body in chunks."""
    u = urllib.parse.urlsplit(url)
    conn = http.client.HTTPConnection(u.hostname, u.port, timeout=timeout)
    body = urllib.parse.urlencode({"query": query})
    conn.request("POST", u.path or "/", body, {"Accept": "application/sparql-results+json",
                                              "Content-Type": "application/x-www-form-urlencoded"})
    resp = conn.getresponse()
    def chunks():
        while True:
            c = resp.read(CHUNK)
            if not c: return
            yield c
    return resp.status, chunks(), conn


# QLever ends its JSON with `,"meta":{"query-time-ms":15,"result-size-total":4,"warnings":[]}}` (captured from a real response): the digits of the
# time vary from run to run, so two correct answers differ in length by a few bytes. The time is not part of the answer: the byte counts used
# to compare a drained run with the hashed check run leave it out.
VOLATILE_TRAILER = re.compile(rb'"query-time-ms"\s*:\s*[-+0-9.eE]+')


class ByteCount:
    """Counts a body's bytes and remembers its last 512, so the volatile trailer can be left out of the count."""
    def __init__(self): self.n, self.tail = 0, b""
    def add(self, chunk): self.n += len(chunk); self.tail = (self.tail + chunk)[-512:]
    def stable(self):
        """The byte count without the volatile trailer."""
        return self.n - sum(m.end() - m.start() for m in VOLATILE_TRAILER.finditer(self.tail))
    def ends_closed(self): return self.tail.rstrip().endswith(b"}")


def drain(chunks, deadline):
    """Reads the body to the last byte without parsing it: returns its byte count (without the volatile trailer) and whether it ends
    like a complete SPARQL JSON document (the closing braces). Used for the timed runs of large answers, where hashing the rows in
    Python is slower than any engine (7.9M rows take about 20 s to fold) and would be what gets timed."""
    count = ByteCount()
    head = b""
    for c in chunks:
        if time.monotonic() > deadline: raise Timeout()
        if len(head) < HEAD_BYTES: head += c[: HEAD_BYTES - len(head)]
        count.add(c)
    # the projected variables come from the head (the first bytes): a drained repeat must have the projection of the hashed check run
    vars_ = head_vars(head.decode("utf-8", "replace"))
    return {"bytes": count.stable(), "tail_ok": count.ends_closed(), "vars": vars_}


def one_run(url, query, timeout, drain_only=False, keep=False):
    """One timed execution to the last byte. Returns (record dict, seconds). With `drain_only` the body is only counted (see `drain`);
    with `keep` the record also carries the full typed answer (up to KEEP_CAP rows) for a case policy."""
    global KEEP_ROWS
    KEEP_ROWS = keep
    try: return _one_run(url, query, timeout, drain_only)
    finally: KEEP_ROWS = False


def _one_run(url, query, timeout, drain_only=False):
    t0 = time.monotonic(); deadline = t0 + timeout
    try:
        status, chunks, conn = post(url, query, timeout)
        try:
            if status != 200:
                body = b"".join(chunks)[:200]
                return {"status": "error", "detail": f"HTTP {status}: {body!r}"}, time.monotonic() - t0
            if drain_only:
                rec = drain(chunks, deadline)
            else:
                count = ByteCount()
                def counted():
                    for c in chunks: count.add(c); yield c
                rec = stream_fold(counted(), deadline)
                rec["bytes"] = count.stable()
        finally:
            conn.close()
        rec["status"] = "ok"
        return rec, time.monotonic() - t0
    except (Timeout, TimeoutError):
        return {"status": "timeout"}, time.monotonic() - t0
    except Truncated as e:
        return {"status": "truncated", "detail": str(e)}, time.monotonic() - t0
    except OSError as e:
        if "timed out" in str(e): return {"status": "timeout"}, time.monotonic() - t0
        return {"status": "error", "detail": f"{type(e).__name__}: {e}"[:200]}, time.monotonic() - t0
    except (ValueError, KeyError) as e:
        return {"status": "error", "detail": f"unparsable answer: {e}"[:200]}, time.monotonic() - t0


# QLever's two-order index (PSO and POS only) refuses a query with a variable predicate with this sentence. Only this text is
# classified `unsupported` (an engine that cannot answer the pattern at all); every other error stays `error`.
UNSUPPORTED_QLEVER_TWO_ORDER = "contains a predicate variable, but only the PSO and POS permutations were loaded"


def classify(rec, ref):
    """pass | value-compatible | mismatch | incomplete | timeout | error, for one run record against the reference."""
    s = rec["status"]
    if s == "timeout": return "timeout"
    if s == "error": return "unsupported" if UNSUPPORTED_QLEVER_TWO_ORDER in rec.get("detail", "") else "error"
    if s == "truncated": return "incomplete"
    # the digests hash values by position, so a renamed or reordered projection would pass them: compare the projected variables first, for every suite
    if "vars" in ref and rec.get("vars") != ref["vars"]: return "mismatch"
    if rec["rows"] == ref["rows"] and rec["bag"] == ref["bag"]: return "pass"
    if rec["rows"] == ref["rows"] and rec["bag_norm"] == ref["bag_norm"]: return "value-compatible"
    if rec["rows"] < ref["rows"] and rec["rows"] == CAP: return "incomplete"
    return "mismatch"


def load_queries(qdir):
    import pathlib
    files = sorted(pathlib.Path(qdir).glob("*.rq"), key=lambda p: (p.stem.rstrip("0123456789"), int("".join(c for c in p.stem if c.isdigit()) or 0)))
    return [(f.stem, f.read_text()) for f in files]


def git_sha():
    try: return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=os.path.dirname(os.path.abspath(__file__)), text=True).strip()
    except Exception: return None


def versions():
    out = {}
    try:
        for line in open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "VERSIONS")):
            if "=" in line and not line.startswith("#"): k, v = line.strip().split("=", 1); out[k] = v
    except OSError: pass
    return out


def cmd_reference(a):
    ref = {}
    for name, q in load_queries(a.queries):
        rec, secs = one_run(a.url, q, a.timeout)
        if rec["status"] != "ok": sys.exit(f"reference run of {name} failed: {rec}")
        rec["ordered"] = "ORDER BY" in q.upper(); ref[name] = rec
        print(f"{name}: {rec['rows']} rows", flush=True)
    json.dump({"reference_url": a.url, "git": git_sha(), "versions": versions(), "queries": ref}, open(a.out, "w"), indent=1)


def run_hook(hook, url):
    """Run the cold-start hook (restart the server, drop the page cache); then wait until the endpoint answers."""
    r = subprocess.run(hook, shell=True, capture_output=True, text=True)
    ok = r.returncode == 0
    # A hook that failed has usually stopped the server (a refused page-cache drop comes after the stop): report at once with its
    # output rather than polling for up to ten minutes for an endpoint that will not come back.
    if not ok: return False, (r.stdout + r.stderr)[-300:]
    deadline = time.time() + 600
    while time.time() < deadline:
        # constant predicate: QLever's two-order index cannot answer a variable predicate, so ASK { ?s ?p ?o } would never succeed there
        rec, _ = one_run(url, "ASK { ?s <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> ?o }", 30)
        if rec["status"] == "ok": break
        time.sleep(2)
    else: ok = False
    return ok, (r.stdout + r.stderr)[-300:]


def restart_after_timeout(a, rec, entry):
    """A client-side timeout leaves the server still working on the query. With --restart-hook the server is restarted (and waited
    for) before the next query, so a timed-out query cannot slow down what follows. The restart is recorded in the entry."""
    if rec["status"] != "timeout" or not a.restart_hook: return
    ok, note = run_hook(a.restart_hook, a.url)
    entry.setdefault("restarts_after_timeout", []).append({"ok": ok, "note": note})


def clear_engine_cache(a, entry):
    """Empties the server's own result cache before a request (QLever caches any result under its size limits, so the -c/-e/-E
    1MB flags do not turn it off: a 10-row aggregate is cached and later requests measure the cache, `docs/qlever-cache-check-v1`).
    A failed clear is recorded, returns False, and the caller does not time that query (a cached answer would be measured); the run
    is then ineligible (`eligibility`). The other queries' results are kept."""
    if not a.clear_cache_url: return True
    try:
        with urllib.request.urlopen(a.clear_cache_url, timeout=60) as r: r.read()
        return True
    except Exception as e:
        entry["cache_clear_failed"] = f"{type(e).__name__}: {e}"[:200]
        return False


GOOD_CLASSES = ("pass", "value-compatible")


def eligibility(records, expected_unsupported=()):
    """Whether a run may be published as a comparison: every query answered correctly, every cold hook and cache clear worked, the
    warm-up was as good as the timed runs, an ordered query came back in the reference order, and nothing was cut off. An
    `unsupported` answer counts only for a case declared in `expected_unsupported` (a known limit of that system's configuration);
    it stays recorded as `unsupported` either way. Returns (eligible, reasons); each reason names the query."""
    reasons = []
    def acceptable(name, cls): return cls in GOOD_CLASSES or (cls == "unsupported" and name in expected_unsupported)
    for name, e in records.items():
        if e.get("cold_hook_ok") is False: reasons.append(f"{name}: cold-start hook failed, not timed")
        if "cache_clear_failed" in e: reasons.append(f"{name}: result-cache clear failed, not timed")
        if not acceptable(name, e["class"]):
            reasons.append(f"{name}: unsupported, not declared with --expect-unsupported" if e["class"] == "unsupported" else f"{name}: {e['class']}")
        warm = e.get("warmup_class")
        if warm is not None and not acceptable(name, warm) and warm != e["class"]:
            reasons.append(f"{name}: warm-up {warm}")
        if e.get("order_matches_reference") is False: reasons.append(f"{name}: ordered query returned a different row order than the reference")
    return (not reasons), reasons


POLICY_CLASSES = ("pass", "value-compatible", "mismatch")


def load_policy(a, scored_manifest=None, reference=None):
    """The case policy named by --case-policy (a Python module inside --policy-bundle), started with the run's verified context, or None.

    The module provides NAME and DATASET_SHA256 (the dataset digest or digests it is bound to), `start(context)` (may raise to refuse the run)
    and `check(case_id, observed_answer, reference_record) -> (class, reason)` with class "pass", "value-compatible" or "mismatch" and a
    non-empty reason string. The driver calls it when the exact comparison failed (including an ordered answer whose rows match but whose
    sequence does not) and, as a typed-projection check, for every empty answer, whose folded digest does not carry the projected variables. `observed_answer` is {"vars": [...], "rows": [{var: SPARQL-JSON binding}]}. `context` is {"case_queries": {case id: sha256 of the
    query file}, "dataset_sha256": [...], "bundle_dir": path, "bundle_sha256": ..., "run_id": ..., "reference_records": {case id: the
    driver's folded reference}, "fold": the Fold class (to verify folded references against frozen typed answers)}: the actual inputs of this run, for the
    policy to verify against its frozen registry. A policy bound to other dataset digests than this run's refuses the run here."""
    if not a.case_policy: return None
    if not a.policy_bundle: sys.exit("--case-policy needs --policy-bundle (the directory holding the policy module, its imports and sidecars)")
    import importlib.util, manifest as mf
    bundle = pathlib.Path(a.policy_bundle).resolve(); file = pathlib.Path(a.case_policy).resolve()
    if bundle not in file.parents: sys.exit("the case policy module must be inside its bundle: the bundle digest covers it and everything it imports")
    spec = importlib.util.spec_from_file_location("case_policy", file); module = importlib.util.module_from_spec(spec)
    sys.path.insert(0, str(bundle)); keep, sys.dont_write_bytecode = sys.dont_write_bytecode, True      # importing must not write a __pycache__ into the bundle it binds
    try: spec.loader.exec_module(module)
    finally: sys.path.remove(str(bundle)); sys.dont_write_bytecode = keep
    for attr in ("NAME", "DATASET_SHA256", "start", "check"):
        if not hasattr(module, attr): sys.exit(f"the case policy lacks {attr}")
    case_queries = mf.query_file_digests(a.queries)
    datasets = [d["sha256"] for d in (scored_manifest or {}).get("datasets", [])]
    bound = module.DATASET_SHA256 if isinstance(module.DATASET_SHA256, (list, tuple)) else [module.DATASET_SHA256]
    if scored_manifest is not None and (not datasets or set(bound) != set(datasets)):
        sys.exit(f"the case policy {module.NAME!r} is bound to dataset digests {sorted(bound)}, not this run's {sorted(datasets)}: it refuses to score")
    context = {"case_queries": case_queries, "dataset_sha256": datasets, "bundle_dir": str(bundle), "bundle_sha256": mf.sha256_tree(bundle),
               "run_id": (scored_manifest or {}).get("run_id"), "reference_records": reference or {}, "fold": Fold}
    try: module.start(context)
    except Exception as e: sys.exit(f"the case policy {module.NAME!r} refuses to start: {e}")
    return module, context


def policy_identity(a):
    """The identity a manifest must record for this run's case policy, or None when the run has none."""
    if not a.case_policy: return None
    import manifest as mf
    if not a.policy_bundle: sys.exit("--case-policy needs --policy-bundle")
    return mf.policy_identity(a.case_policy, a.policy_bundle, mf.query_file_digests(a.queries))


def check_manifest(a):
    """(scored, info): a run is scored only with a complete manifest and a fresh passing machine receipt (manifest.py). `--scored`
    refuses to start without them; otherwise the results file says `scored: false` and the report will not compare it."""
    if not a.manifest:
        if a.scored: sys.exit("--scored needs --manifest (see bench/eval/manifest.py write)")
        return False, {"scored_reason": "no manifest"}
    import manifest as mf
    if a.mode == "cold":
        # A cold run is scored only under the plan runner, which invokes this repository's `restart.sh --cold` itself: the hook must be exactly
        # the one the manifest records (the parser accepts only that script with that one argument) and the digest of restart.sh and lib.sh
        # must be unchanged. `--plan-runner RUN_ID` must name the manifest's run, so a hand-made command line cannot claim it by accident.
        if not a.plan_runner: 
            if a.scored: sys.exit("cold scoring needs the plan runner: --scored --mode cold is refused outside bench/eval/plan.py")
            return False, {"manifest": str(a.manifest), "scored_reason": "cold runs are scored only by the plan runner"}
    try: m, problems = mf.load_checked(a.manifest)
    except Exception as e: m, problems = {}, [f"cannot read the manifest: {e}"]
    try: lacking = sorted(q for q, rec in json.load(open(a.reference))["queries"].items() if "vars" not in rec)
    except Exception as e: lacking = [f"unreadable reference ({e})"]
    if lacking: problems = problems + [f"reference lacks projection (`vars`) for {len(lacking)} case(s), e.g. {lacking[:3]}: regenerate it from its source system"]
    if not problems:
        # the manifest must describe this run: its query set, reference, engine, protocol, mode and expected-unsupported policy
        problems = mf.run_bindings(m, queries_sha256=mf.sha256_tree(a.queries), reference_sha256=mf.sha256_file(a.reference), system=a.system, url=a.url, clear_cache_url=a.clear_cache_url, runs=a.runs,
                                   timeout_s=a.timeout, seed=a.seed, drain_rows=a.drain_rows, mode=a.mode, cold_hook=a.cold_hook or "",
                                   expect_unsupported=[x for x in re.split(r"[,\s]+", (a.expect_unsupported or "").strip()) if x], policy=policy_identity(a))
    if not problems and a.mode == "cold" and a.plan_runner != m.get("run_id"):
        problems = [f"--plan-runner {a.plan_runner!r} is not this manifest's run ({m.get('run_id')!r})"]
    if problems:
        if a.scored: sys.exit("run is not scorable:\n  " + "\n  ".join(problems))
        return False, {"manifest": str(a.manifest), "scored_reason": problems}
    a._manifest = m
    return True, {"manifest": str(a.manifest), "manifest_sha256": mf.sha256_file(a.manifest), "run_id": m.get("run_id")}


def judge(policy, name, rec, r):
    """(class, reason) from the case policy for one checked answer, or None when the policy is not consulted: only when the exact comparison
    failed (rows, bag and, for an ordered query, sequence) or the answer is empty (the typed-projection check). A verdict the policy may not
    give is a mismatch with that reason: `pass` only for an exact empty answer, never for an inexact one."""
    if policy is None or rec.get("status") != "ok": return None
    if "vars" in r and rec.get("vars") != r["vars"]: return ("mismatch", f"the projected variables {rec.get('vars')} differ from the reference's {r['vars']}")
    exact = rec["rows"] == r["rows"] and rec["bag"] == r["bag"] and (not r.get("ordered") or rec["seq"] == r["seq"])
    if exact and rec["rows"] > 0: return None
    answer = rec.get("answer")
    if answer is None: return ("mismatch", f"the answer was not kept for the case policy (more than {KEEP_CAP} rows)")
    try: cls, reason = policy[0].check(name, answer, r)
    except Exception as e: return ("mismatch", f"the case policy failed on this answer: {type(e).__name__}: {e}"[:300])
    if cls not in POLICY_CLASSES or not isinstance(reason, str) or not reason: return ("mismatch", "the case policy returned an invalid verdict")
    if cls == "pass" and not exact: return ("mismatch", "the case policy returned pass for an inexact answer")
    if cls == "value-compatible" and exact: return ("mismatch", "the case policy relaxed an exact answer")
    return (cls, reason)


def cmd_run(a):
    scored, scored_info = check_manifest(a)
    ref = json.load(open(a.reference))["queries"]
    policy = load_policy(a, getattr(a, "_manifest", None) if scored else None, ref)
    if a.case_policy and not scored: scored_info = {**scored_info, "scored_reason": scored_info.get("scored_reason", "a case policy needs a manifest that binds it")}
    records = {}
    rng = random.Random(a.seed)
    queries = load_queries(a.queries)
    for name, q in queries:
        if name not in ref: sys.exit(f"{name} has no reference answer")
        r = ref[name]; entry = {"mode": a.mode, "runs": []}
        checked = None          # (record, (class, reason)) when the case policy judged the checked answer
        if a.mode == "cold":
            if not a.cold_hook: sys.exit("--mode cold needs --cold-hook")
            # one reset before every requested repeat: N cold samples per query. A reset that fails ends the query: it is not-run.
            entry["cold_hook_ok"] = True
            runs = a.runs
        elif not clear_engine_cache(a, entry):
            runs = 0
        else:
            check, _ = one_run(a.url, q, a.timeout, keep=policy is not None)       # warm-up; for a large answer it is also the one answer check
            v = judge(policy, name, check, r)
            checked = (check, v) if v else None
            if checked: entry["warmup_class"] = checked[1][0]
            else: entry["warmup_class"] = classify(check, r)       # kept even when every timed run then passes: a warm-up timeout is a finding
            restart_after_timeout(a, check, entry)
            runs = a.runs
            # Large answers: the timed runs only drain the body (time to the last byte); the rows were hashed once above, and a
            # drained run must return exactly as many bytes as that check or it is recorded as unstable.
            if r["rows"] >= a.drain_rows:
                check_cls = classify(check, r)
                if v:      # a case the policy had to judge cannot be re-validated on drained repeats (only their bytes are compared): it is not timed
                    check_cls = "mismatch"
                    entry.setdefault("policy_reasons", []).append("the case policy judged this answer, which is too large to validate on drained timed repeats: " + v[1])
                entry["timed_by"] = "drain"
                entry["check_class"] = check_cls
                if check_cls not in ("pass", "value-compatible"): runs = 0      # a wrong or incomplete answer is never timed
                else: drain_bytes = check["bytes"]
        for _ in range(runs):
            if a.mode == "cold":
                ok, note = run_hook(a.cold_hook, a.url)
                entry["cold_hook_note"] = note
                if not ok: entry["cold_hook_ok"] = False; entry["runs"] = []; break      # no timed sample of this query survives a failed reset
            if not clear_engine_cache(a, entry): break
            t0 = time.time()   # wall-clock bounds of the run, so S1's cgroup samples can be joined per query (s1-report.py)
            drained = entry.get("timed_by") == "drain"
            first_cold = a.mode == "cold" and not entry["runs"]
            rec, secs = one_run(a.url, q, a.timeout, drain_only=drained, keep=policy is not None and first_cold)
            t1 = time.time()
            restart_after_timeout(a, rec, entry)
            if first_cold and policy is not None:
                v = judge(policy, name, rec, r)
                if v: checked = (rec, v)
            if drained and rec["status"] == "ok":
                cls = entry["check_class"] if rec["bytes"] == drain_bytes and rec["tail_ok"] and rec.get("vars") is not None and rec.get("vars") == check.get("vars") else "unstable"
                rec = {**rec, **{k: check[k] for k in ("rows", "bag", "seq") if k in check}}
            else:
                cls = classify(rec, r)
                if policy is not None and checked and rec.get("status") == "ok" and rec.get("rows") == 0 and rec.get("vars") != checked[0].get("vars"):
                    cls = "mismatch"      # an empty answer's digest carries no values: its projection must be the one the policy checked
                    entry.setdefault("policy_reasons", []).append(f"the projected variables {rec.get('vars')} differ from the checked answer's {checked[0].get('vars')}")
                elif checked and rec.get("status") == "ok" and all(rec.get(k) == checked[0].get(k) for k in ("rows", "bag", "seq", "vars")):
                    cls = checked[1][0]                      # the same answer the case policy judged: it keeps the policy's class
                    entry.setdefault("policy_reasons", [])
                    if checked[1][1] not in entry["policy_reasons"]: entry["policy_reasons"].append(checked[1][1])
            entry["runs"].append({"class": cls, "seconds": round(secs, 4), "t0": round(t0, 3), "t1": round(t1, 3), **{k: rec[k] for k in ("rows", "bag", "seq", "bytes") if k in rec},
                                  **({"detail": rec["detail"]} if "detail" in rec else {})})
        if entry.get("timed_by") == "drain" and not entry["runs"] and entry["check_class"] not in GOOD_CLASSES:
            # a wrong or incomplete large answer is never timed; its class is recorded (without a time) for the report
            entry["runs"].append({"class": entry["check_class"], "seconds": None, **{k: check[k] for k in ("rows", "bag", "seq", "bytes") if k in check}})
        classes = {x["class"] for x in entry["runs"]}
        if not entry["runs"]: entry["class"] = "not-run"     # a failed cold hook or cache clear: never a timing row
        else: entry["class"] = classes.pop() if len(classes) == 1 else "unstable"
        ok_runs = [x["seconds"] for x in entry["runs"] if x["class"] in GOOD_CLASSES and x["seconds"] is not None]
        if ok_runs: entry["median_s"] = round(statistics.median(ok_runs), 4)
        if r.get("ordered") and entry["class"] in ("pass", "value-compatible"):
            accepted = checked[0].get("seq") if checked and checked[1][0] in ("pass", "value-compatible") else None      # a sequence the case policy accepted, with its reason
            entry["order_matches_reference"] = all(x.get("seq") == r["seq"] or (accepted is not None and x.get("seq") == accepted) for x in entry["runs"])
        records[name] = entry
        print(f"{name:<8} {entry['class']:<17} {entry.get('median_s', '-')}", flush=True)
    summary = {}
    for e in records.values(): summary[e["class"]] = summary.get(e["class"], 0) + 1
    eligible, reasons = eligibility(records, set(re.split(r"[,\s]+", a.expect_unsupported.strip())) if a.expect_unsupported else ())
    json.dump({"system": a.system, "scored": scored and eligible, **({"case_policy": policy_identity(a)} if a.case_policy else {}), "eligible": eligible, "ineligible_reasons": reasons, **scored_info, "url": a.url, "mode": a.mode, "timeout_s": a.timeout, "runs_per_query": a.runs, "seed": a.seed,
               "git": git_sha(), "versions": versions(), "python": platform.python_version(), "platform": platform.platform(),
               "summary": summary, "queries": records}, open(a.out, "w"), indent=1)
    print("summary:", summary)
    if not eligible:
        print("INELIGIBLE for a comparison:", *reasons, sep="\n  ", file=sys.stderr)
        if not a.lenient: sys.exit(3)       # the results file is already written: the logs of a failed run are kept


# ---------------------------------------------------------------- self-test

def _bindings(n, kind="ok"):
    for i in range(n):
        dt = f',"datatype":"{XSD}{"int" if kind == "int" else "integer"}"'
        v = i + (1 if kind == "wrong" and i == 0 else 0)
        yield f'{{"s":{{"type":"uri","value":"urn:s{i}"}},"n":{{"type":"literal","value":"{v}"{dt}}}}}'


def _body(n, kind="ok"):
    yield '{"head":{"vars":["s","n"]},"results":{"bindings":[\n'
    first = True
    for b in _bindings(n, kind):
        yield ("" if first else ",\n") + b; first = False
    yield "\n]}}"


def _body_qtime(n, ms):
    """The ok body with the `meta` member QLever appends, whose `query-time-ms` has a length that depends on `ms`."""
    text = "".join(_body(n))
    assert text.endswith("\n]}}")
    return text[:-1] + f',"meta":{{"query-time-ms":{ms},"result-size-total":{n},"warnings":[]}}}}'


BYCASE = {}      # case name -> the SPARQL JSON the fake server answers it with (the case-policy tests)
_ONE_ROW_9 = '{"head":{"vars":["n"]},"results":{"bindings":[{"n":{"type":"literal","value":"9","datatype":"' + XSD + 'integer"}}]}}'


def serve_fake(behaviors):
    import threading, http.server
    class H(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a): pass
        def do_GET(self):
            # cache-clear endpoints: /clear-once works the first time and fails afterwards, /clear-ok always works
            n = self.server.clears = getattr(self.server, "clears", 0) + (self.path == "/clear-once")
            code = 500 if (self.path == "/clear-once" and n > 1) or self.path not in ("/clear-once", "/clear-ok") else 200
            self.send_response(code); self.end_headers(); self.wfile.write(b"x")
        def do_POST(self):
            self.body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            kind, n = behaviors[self.path]
            if kind == "err": self.send_response(500); self.end_headers(); self.wfile.write(b"boom"); return
            if kind == "unsup": self.send_response(500); self.end_headers(); self.wfile.write(b'{"exception": "The query contains a predicate variable, but only the PSO and POS permutations were loaded. Rerun the server without the option."}'); return
            self.send_response(200); self.send_header("Content-Type", "application/sparql-results+json"); self.end_headers()
            if kind == "slow": time.sleep(3)
            if kind == "ask": self.wfile.write(b'{"head":{},"boolean":true}'); return
            if kind in ("asc", "desc"):
                rows = ["1", "2"] if kind == "asc" else ["2", "1"]
                self.wfile.write(('{"head":{"vars":["n"]},"results":{"bindings":[%s]}}' % ",".join('{"n":{"type":"literal","value":"%s","datatype":"%sinteger"}}' % (v, XSD) for v in rows)).encode()); return
            if kind == "raw": self.wfile.write(n.encode()); return
            if kind == "flipvar":      # a non-empty answer: the right projection on the first request, a same-length renamed one on every later request
                self.server.flipv = getattr(self.server, "flipv", 0) + 1
                self.wfile.write((BYCASE["q_one"] if self.server.flipv == 1 else BYCASE["q_one_renamed"]).encode()); return
            if kind == "flipempty":      # an empty answer with the right projection on the first request and a wrong one on every later request
                self.server.flips = getattr(self.server, "flips", 0) + 1
                self.wfile.write((BYCASE["q_empty"] if self.server.flips == 1 else BYCASE["q_empty_wrong"]).encode()); return
            if kind == "bycase":      # the answer for the case named in the query text (`# case: NAME`), from BYCASE
                query = urllib.parse.parse_qs(self.body.decode())["query"][0]
                self.wfile.write(BYCASE[re.search(r"# case: (\S+)", query).group(1)].encode()); return
            if kind == "slowfirst":      # the first request outlasts the client's timeout, the rest answer at once
                self.server.firsts = getattr(self.server, "firsts", 0) + 1
                if self.server.firsts == 1: time.sleep(3)
                self.wfile.write(_ONE_ROW_9.encode()); return      # a recorded answer, served as it was
            if kind == "count9s":
                self.wfile.write(b'{"head":{"vars":["n"]},"results":{"bindings":[{"n":{"type":"literal","value":"9"}}]}}'); return
            if kind == "twice":
                row = '{"n":{"type":"literal","value":"9","datatype":"%sinteger"}}' % XSD
                self.wfile.write(('{"head":{"vars":["n"]},"results":{"bindings":[%s,%s]}}' % (row, row)).encode()); return
            if kind.startswith("count="):       # one aggregate row, an xsd:integer of the given value
                self.wfile.write(('{"head":{"vars":["n"]},"results":{"bindings":[{"n":{"type":"literal","value":"%s","datatype":"%sinteger"}}]}}' % (kind[6:], XSD)).encode()); return
            if kind == "qtime":
                self.qtime_calls = getattr(self.server, "qtime_calls", 0) + 1; self.server.qtime_calls = self.qtime_calls
                text = _body_qtime(n, 7 if self.qtime_calls % 2 else 12345)   # alternating digit counts
            else: text = "".join(_body(n, kind if kind in ("int", "wrong") else "ok"))
            if kind == "trunc": text = text[: len(text) // 2]
            self.wfile.write(text.encode())
    srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv, srv.server_address[1]


STUB_POLICY = """
NAME = "stub-bi"
DATASET_SHA256 = ["%s"]
TYPED_REFERENCE = {
    "q_interval": {"vars": ["v"], "rows": [[("float", "1.0")]]},
    "q_lexical": {"vars": ["v"], "rows": [[("float", "1.0")]]},
    "q_cutoff": {"vars": ["p"], "rows": [[("uri", "a")], [("uri", "b")], [("uri", "c")]], "ties": ["c", "d"], "open_slots": 1},
    "q_order_ok": {"vars": ["v"], "rows": [[("integer", "1")], [("integer", "2")]], "ties": ["1", "2"]},
    "q_order_bad": {"vars": ["v"], "rows": [[("integer", "1")], [("integer", "2")]]},
    "q_empty": {"vars": ["x"], "rows": []},
}
started = {}

def start(context):
    if not set(context["case_queries"]) <= set(TYPED_REFERENCE): raise RuntimeError("the run has cases the policy has not frozen")
    started["context"] = context

def _cells(answer):
    return [[(b["type"] if b["type"] != "literal" else b["datatype"].rsplit("#", 1)[-1], b["value"]) for b in (row.get(v) for v in answer["vars"])] for row in answer["rows"]]

def check(case_id, observed, reference_record):
    ref, rows = TYPED_REFERENCE[case_id], _cells(observed)
    if observed["vars"] != ref["vars"]: return "mismatch", "projected variables %%s differ from the frozen %%s" %% (observed["vars"], ref["vars"])
    if case_id == "q_empty": return ("pass", "the typed projection matches") if not rows else ("mismatch", "rows where none are registered")
    if case_id == "q_interval":
        (kind, value), = rows[0] if len(rows) == 1 else [(None, "x")]
        if kind != "float": return "mismatch", "not an xsd:float"
        return ("value-compatible", "within the registered interval") if abs(float(value) - 1.0) <= 1e-3 else ("mismatch", "outside the registered interval")
    if case_id == "q_lexical":
        return ("value-compatible", "lexical form differs at an unchanged binary32 value") if len(rows) == 1 and rows[0][0][0] == "float" and float(rows[0][0][1]) == 1.0 else ("mismatch", "value changed")
    if case_id == "q_cutoff":
        kept = [r[0][1] for r in rows]
        if len(kept) != 3 or len(set(kept)) != 3 or kept[:2] != ["a", "b"] or kept[2] not in ref["ties"]: return "mismatch", "not a valid cutoff selection"
        return "value-compatible", "valid selection among the products tied at the cutoff"
    if case_id == "q_order_ok": return ("value-compatible", "order differs only inside a registered tie") if sorted(rows) == sorted(ref["rows"]) else ("mismatch", "rows differ")
    if case_id == "q_order_bad": return "mismatch", "the registered order is strict"
    return "mismatch", "unknown case"
"""


HERE_DIR = pathlib.Path(__file__).resolve().parent


def gate_tests(port, behaviors_raw):
    """The correctness gate end to end: the driver as a subprocess against fake servers, checking the results file and the exit
    status. A run that is wrong, cut off, timed out or whose cold start failed is ineligible, is not timed where timing would
    mislead, and fails with status 3 (unless `--lenient`) after writing its results file."""
    import tempfile, pathlib
    ok = True
    with tempfile.TemporaryDirectory() as tmp:
        tmp = pathlib.Path(tmp); qdir = tmp / "q"; qdir.mkdir()
        (qdir / "q1.rq").write_text("SELECT * WHERE {}")
        def reference(count):       # the reference answer: one integer aggregate
            doc = '{"head":{"vars":["n"]},"results":{"bindings":[{"n":{"type":"literal","value":"%s","datatype":"%sinteger"}}]}}' % (count, XSD)
            return {"queries": {"q1": {**fold_of([doc]), "ordered": False}}}
        (tmp / "ref9.json").write_text(json.dumps(reference(9)))
        def run(name, path, *extra, ref="ref9.json", url=None):
            out = tmp / f"{name}.json"
            cmd = [sys.executable, __file__, "run", "--system", name, "--url", url or f"http://127.0.0.1:{port}/{path}", "--queries", str(qdir),
                   "--reference", str(tmp / ref), "--out", str(out), "--runs", "2", "--timeout", "30", *extra]
            r = subprocess.run(cmd, capture_output=True, text=True)
            return r.returncode, (json.load(open(out)) if out.exists() else None), r.stderr
        def check(label, good):
            nonlocal ok
            print(f"gate     {label:<58} {'ok' if good else 'FAIL'}"); ok &= good
        # Jianlin's regression: Ontop/DuckDB returned 8 for a distinct aggregate where Jena returned 9. Typed bindings must differ.
        rc, res, _ = run("ontop", "count8")
        e = res["queries"]["q1"] if res else {}
        check("8 against a reference 9: mismatch, ineligible, exit 3", rc == 3 and res and res["eligible"] is False and e.get("class") == "mismatch")
        check("8 against 9: no timing row (no median)", "median_s" not in e)
        rc, res, _ = run("jena", "count9")
        check("9 against 9: pass, eligible, exit 0", rc == 0 and res and res["eligible"] and res["queries"]["q1"]["class"] == "pass")
        # The recorded replay: the answer Ontop/DuckDB gave in the diagnostic packet (`docs/ontop-distinct-diagnostic-v3`), served as
        # recorded, against the nine singleton term groups Jena returned (one binding ?n = 9). It must not pass and must have no median.
        packet = pathlib.Path(__file__).resolve().parents[2] / "docs" / "ontop-distinct-diagnostic-v3" / "packet.tar.gz"
        if packet.exists():
            import tarfile
            with tarfile.open(packet) as t:
                observed = json.dumps(json.load(t.extractfile("attempt/original/child/distinct_count.json"))["answer"])
            behaviors_raw["/ontop-replay"] = ("raw", observed)
            rc, res, _ = run("ontop-replay", "ontop-replay")
            e = res["queries"]["q1"] if res else {}
            check("recorded Ontop/DuckDB answer (8) against Jena's 9: no pass, no median", rc == 3 and e.get("class") == "mismatch" and "median_s" not in e)
        else: print("gate     recorded Ontop replay skipped: packet not in this checkout")
        # the same value with a different datatype is a different typed binding (value-compatible only for integer-like widening)
        rc, res, _ = run("typed", "count9s")
        check("a string '9' against an integer 9: not a pass", res and res["queries"]["q1"]["class"] != "pass")
        # multiplicity: the same rows twice are not the same bag
        rc, res, _ = run("twice", "twice")
        check("a duplicated row: mismatch, exit 3", rc == 3 and res and res["queries"]["q1"]["class"] == "mismatch")
        # a cold hook that fails: nothing is timed, the run is ineligible, the results file is kept
        rc, res, _ = run("coldfail", "count9", "--mode", "cold", "--cold-hook", "false")
        e = res["queries"]["q1"] if res else {}
        check("a failed cold hook: not-run, no timed run, ineligible, exit 3", rc == 3 and e.get("class") == "not-run" and not e.get("runs") and "median_s" not in e)
        # a refused reset has stopped the server: the failure is reported at once, not after polling for an endpoint that never returns
        t0 = time.time(); rc, res, _ = run("coldgone", "x", "--mode", "cold", "--cold-hook", "echo reset refused >&2; false", url="http://127.0.0.1:9/x")
        e = res["queries"]["q1"] if res else {}
        check("a failed hook with the server gone returns within 20 s with its output", time.time() - t0 < 20 and rc == 3 and "reset refused" in e.get("cold_hook_note", ""))
        rc, res, _ = run("coldok", "count9", "--mode", "cold", "--cold-hook", "true")
        check("a working cold hook: one reset and one timed sample per requested repeat (2), eligible, exit 0", rc == 0 and res and res["queries"]["q1"]["class"] == "pass" and len(res["queries"]["q1"]["runs"]) == 2)
        counter = tmp / "resets"
        flaky = f'n=$(cat "{counter}" 2>/dev/null || echo 0); echo $((n+1)) > "{counter}"; [ "$n" -lt 1 ]'       # the first reset works, the second fails
        rc, res, _ = run("coldmid", "count9", "--mode", "cold", "--cold-hook", flaky)
        e = res["queries"]["q1"] if res else {}
        check("a reset that fails before the second repeat: the query is not-run, no timing survives, exit 3", rc == 3 and e.get("class") == "not-run" and not e.get("runs") and "median_s" not in e and e.get("cold_hook_ok") is False)
        # a failing cache clear: not timed, recorded, the results file still written
        rc, res, _ = run("clearfail", "count9", "--clear-cache-url", "http://127.0.0.1:9/none")
        e = res["queries"]["q1"] if res else {}
        check("a failed cache clear: not-run, recorded, ineligible, exit 3", rc == 3 and e.get("class") == "not-run" and "cache_clear_failed" in e)
        # with --lenient (exploration only) the exit status stays 0, but the results file still says ineligible
        rc, res, _ = run("lax", "count8", "--lenient")
        check("with --lenient: exit 0, results still say ineligible", rc == 0 and res and res["eligible"] is False)
        # Andy's review of #295: a large answer (drained) passes its hash check, then the cache clear fails before the first timed run.
        # No timing row, no crash, and the file is written: the run is not-run and ineligible.
        rc, res, err = run("drainclear", "count9", "--drain-rows", "1", "--clear-cache-url", f"http://127.0.0.1:{port}/clear-once")
        e = res["queries"]["q1"] if res else {}
        check("drained check passes, then cache clear fails: file written, not-run, exit 3", rc == 3 and res is not None and e.get("class") == "not-run" and "median_s" not in e and "Traceback" not in err)
        # a warm-up that times out, followed by timed runs that pass, is still ineligible and the warm-up outcome is kept
        rc, res, _ = run("warmtimeout", "slowfirst", "--timeout", "1")
        e = res["queries"]["q1"] if res else {}
        check("warm-up timeout then passing timed runs: warmup_class kept, ineligible, exit 3", rc == 3 and e.get("warmup_class") == "timeout" and e.get("class") == "pass" and res["eligible"] is False)
        # unsupported counts only where the case is declared for that system configuration, and is recorded as unsupported either way
        rc, res, _ = run("unsup", "unsup")
        check("an undeclared unsupported answer: class kept, ineligible, exit 3", rc == 3 and res and res["queries"]["q1"]["class"] == "unsupported" and res["eligible"] is False)
        rc, res, _ = run("unsupdecl", "unsup", "--expect-unsupported", "q1")
        check("a declared unsupported answer: class kept, eligible, exit 0", rc == 0 and res and res["queries"]["q1"]["class"] == "unsupported" and res["eligible"])
        # an ordered query that returns the right rows in the wrong order: the bag matches, the order does not
        doc = lambda vals: '{"head":{"vars":["n"]},"results":{"bindings":[%s]}}' % ",".join('{"n":{"type":"literal","value":"%s","datatype":"%sinteger"}}' % (v, XSD) for v in vals)
        (tmp / "refordered.json").write_text(json.dumps({"queries": {"q1": {**fold_of([doc(["1", "2"])]), "ordered": True}}}))
        rc, res, _ = run("orderok", "asc", ref="refordered.json")
        check("an ordered query in the reference order: eligible, exit 0", rc == 0 and res and res["eligible"] and res["queries"]["q1"].get("order_matches_reference") is True)
        rc, res, _ = run("orderbad", "desc", ref="refordered.json")
        check("an ordered query in a different order: ineligible, exit 3", rc == 3 and res and res["eligible"] is False and res["queries"]["q1"].get("order_matches_reference") is False)
        # scored runs: a manifest and a fresh passing machine receipt, or the run does not start
        import manifest as mf
        ndate = datetime.datetime.now(datetime.timezone.utc)
        mdir = tmp / "m"; mdir.mkdir()
        rec = mf.good_receipt(ndate, mf.boot_id()); good = mf.good_manifest(rec)
        good["queries_sha256"] = mf.sha256_tree(qdir); good["reference_sha256"] = mf.sha256_file(tmp / "ref9.json")
        good["protocol"].update(runs=2, timeout_s=30, seed=0, drain_rows=200000)
        good["systems"] = [{"name": n, "version": "t", "config_sha256": "1" * 64, "url": f"http://127.0.0.1:{port}/count9", "clear_cache_url": ""} for n in ("scoredok", "scoredstale", "unscored", "swapped")]
        (mdir / "machine-check.json").write_text(json.dumps(rec)); good["machine"]["receipt_sha256"] = mf.sha256_file(mdir / "machine-check.json")
        (mdir / "manifest.json").write_text(json.dumps(good))
        rc, res, err = run("scoredok", "count9", "--manifest", str(mdir / "manifest.json"), "--scored")
        check("--scored with a complete manifest and fresh receipt: runs, scored: true", rc == 0 and res and res["scored"] is True and res.get("run_id") == "t")
        # the manifest of one run must not be accepted for another: a different reference, an unlisted engine, a different repeat count
        rc, res, err = run("swapped", "count9", "--manifest", str(mdir / "manifest.json"), "--scored", "--runs", "3")
        check("--scored with a different repeat count than the manifest: refused", rc != 0 and res is None and "runs differs" in err)
        rc, res, err = run("stranger", "count9", "--manifest", str(mdir / "manifest.json"), "--scored")
        check("--scored for an engine the manifest does not list: refused", rc != 0 and res is None and "not listed" in err)
        (tmp / "ref8.json").write_text(json.dumps(reference(8)))
        rc, res, err = run("swapped", "count9", "--manifest", str(mdir / "manifest.json"), "--scored", ref="ref8.json")
        check("--scored against a different reference: refused", rc != 0 and res is None and "reference digest" in err)
        rc, res, err = run("swapped", "count9", "--manifest", str(mdir / "manifest.json"), "--scored", url=f"http://127.0.0.1:{port}/count8")
        check("--scored against a different endpoint than the manifest's: refused", rc != 0 and res is None and "endpoint url" in err)
        rc, res, err = run("unscored", "count9", "--manifest", str(mdir / "manifest.json"), "--scored", "--clear-cache-url", f"http://127.0.0.1:{port}/clear-ok")
        check("--scored with a cache-clear url the manifest does not record: refused", rc != 0 and res is None and "clear url" in err)
        # cold scoring: only this repository's reset procedure, bound by command and by the digest of restart.sh and lib.sh
        hook = f"'{pathlib.Path(__file__).resolve().parent / 'restart.sh'}' --cold"
        cold = json.loads(json.dumps(good)); cold["protocol"].update(mode="cold", cold_hook=hook, cold_hook_sha256=mf.cold_hook_digest(hook))
        (mdir / "manifest.json").write_text(json.dumps(cold))
        for label, h in (("`true`", "true"), ("the recorded reset itself", hook)):
            rc, res, err = run("unscored", "count9", "--mode", "cold", "--cold-hook", h, "--manifest", str(mdir / "manifest.json"), "--scored")
            check(f"--scored --mode cold with {label}: refused, the plan runner owns cold scoring", rc != 0 and res is None and "plan runner" in err)
        rc, res, err = run("unscored", "count9", "--mode", "cold", "--cold-hook", hook, "--manifest", str(mdir / "manifest.json"), "--scored", "--plan-runner", "someone-elses-run")
        check("--scored --mode cold under a plan-runner id that is not the manifest's: refused", rc != 0 and res is None and "not this manifest's run" in err)
        # the right id with the recorded reset passes the manifest check; restart.sh then fails here (no SYSTEM), so nothing is timed
        rc, res, err = run("unscored", "count9", "--mode", "cold", "--cold-hook", hook, "--manifest", str(mdir / "manifest.json"), "--scored", "--plan-runner", "t")
        e = res["queries"]["q1"] if res else {}
        check("--scored --mode cold under the plan runner with the recorded reset: accepted, and a failing reset still times nothing", res is not None and res.get("manifest_sha256") and e.get("class") == "not-run" and rc == 3)
        rc, res, err = run("unscored", "count9", "--mode", "cold", "--cold-hook", "true", "--manifest", str(mdir / "manifest.json"))
        check("an unscored cold run with a manifest records why it is not scored", rc == 0 and res and res["scored"] is False and "plan runner" in str(res.get("scored_reason")))
        (mdir / "manifest.json").write_text(json.dumps(good))
        rc, res, err = run("scoredno", "count9", "--scored")
        check("--scored without a manifest: refused, nothing written", rc != 0 and res is None)
        old = mf.good_receipt(ndate - datetime.timedelta(hours=30), mf.boot_id()); (mdir / "machine-check.json").write_text(json.dumps(old))
        good["machine"]["receipt_sha256"] = mf.sha256_file(mdir / "machine-check.json"); (mdir / "manifest.json").write_text(json.dumps(good))
        rc, res, err = run("scoredstale", "count9", "--manifest", str(mdir / "manifest.json"), "--scored")
        check("--scored with a 30-hour-old receipt: refused, nothing written", rc != 0 and res is None and "old" in err)
        rc, res, _ = run("unscored", "count9", "--manifest", str(mdir / "manifest.json"))
        check("an unscored run with a stale receipt runs, but scored: false with the reason", rc == 0 and res and res["scored"] is False and res["eligible"] is True and bool(res.get("scored_reason")))
        rc, res, _ = run("nomanifest", "count9")
        check("a run without a manifest: scored: false", rc == 0 and res and res["scored"] is False)
        # ---- the case policy: answers the exact comparison rejects, judged by a registered policy; every relaxation carries its reason
        import manifest as mf2
        float_ = lambda v: {"type": "literal", "value": v, "datatype": XSD + "float"}
        int_ = lambda v: {"type": "literal", "value": v, "datatype": XSD + "integer"}
        uri_ = lambda v: {"type": "uri", "value": v}
        doc_ = lambda vars_, rows: json.dumps({"head": {"vars": vars_}, "results": {"bindings": [dict(zip(vars_, r)) for r in rows]}})
        ref_answers = {"q_interval": doc_(["v"], [[float_("1.0")]]), "q_lexical": doc_(["v"], [[float_("1.0")]]), "q_cutoff": doc_(["p"], [[uri_("a")], [uri_("b")], [uri_("c")]]),
                       "q_order_ok": doc_(["v"], [[int_("1")], [int_("2")]]), "q_order_bad": doc_(["v"], [[int_("1")], [int_("2")]]), "q_empty": doc_(["x"], [])}
        pdir = tmp / "pq"; pdir.mkdir()
        for c in ref_answers: (pdir / f"{c}.rq").write_text(f"# case: {c}\nSELECT * WHERE {{}}" + (" ORDER BY ?v" if c.startswith("q_order") else ""))
        (tmp / "refpolicy.json").write_text(json.dumps({"queries": {c: {**fold_of([d]), "ordered": c.startswith("q_order")} for c, d in ref_answers.items()}}))
        bundle = tmp / "bundle"; bundle.mkdir(); dataset_digest = "c" * 64
        (bundle / "policy.py").write_text(STUB_POLICY % dataset_digest)
        def served(**over):
            BYCASE.clear(); BYCASE.update(ref_answers); BYCASE.update(over)
        def prun(name, *extra, qd=None, ref="refpolicy.json", policy=True):
            out = tmp / f"{name}.json"
            cmd = [sys.executable, __file__, "run", "--system", name, "--url", f"http://127.0.0.1:{port}/bycase", "--queries", str(qd or pdir), "--reference", str(tmp / ref),
                   "--out", str(out), "--runs", "2", "--timeout", "30", *(["--case-policy", str(bundle / "policy.py"), "--policy-bundle", str(bundle)] if policy else []), *extra]
            r = subprocess.run(cmd, capture_output=True, text=True)
            return r.returncode, (json.load(open(out)) if out.exists() else None), r.stderr
        behaviors_raw["/bycase"] = ("bycase", 0)
        served(q_interval=doc_(["v"], [[float_("1.0004")]]), q_lexical=doc_(["v"], [[float_("1")]]), q_cutoff=doc_(["p"], [[uri_("a")], [uri_("b")], [uri_("d")]]),
               q_order_ok=doc_(["v"], [[int_("2")], [int_("1")]]), q_order_bad=doc_(["v"], [[int_("2")], [int_("1")]]), q_empty=doc_(["y"], []))
        rc, res, err = prun("policy1", "--lenient")
        q = res["queries"] if res else {}
        check("policy: a value inside the registered interval is value-compatible, with the policy's reason", q.get("q_interval", {}).get("class") == "value-compatible" and q["q_interval"].get("policy_reasons") == ["within the registered interval"])
        check("policy: a lexical-only difference is value-compatible, with its reason", q.get("q_lexical", {}).get("class") == "value-compatible" and "lexical" in q["q_lexical"]["policy_reasons"][0])
        check("policy: a valid cutoff selection among tied products is value-compatible", q.get("q_cutoff", {}).get("class") == "value-compatible" and "tied" in q["q_cutoff"]["policy_reasons"][0])
        check("policy: an ordered answer with the right rows in a registered tie order is value-compatible and its sequence is accepted", q.get("q_order_ok", {}).get("class") == "value-compatible" and q["q_order_ok"].get("order_matches_reference") is True)
        check("policy: an ordered answer reordered where the order is strict is a mismatch, ineligible", q.get("q_order_bad", {}).get("class") == "mismatch" and "median_s" not in q["q_order_bad"])
        check("policy: an empty answer with a different projected variable is a mismatch even though the folded digests are equal", q.get("q_empty", {}).get("class") == "mismatch" and fold_of([ref_answers["q_empty"]])["bag"] == fold_of([doc_(["y"], [])])["bag"])
        check("policy: the results carry the policy's identity", res and res.get("case_policy", {}).get("name") == "stub-bi" and len(res["case_policy"]["bundle_sha256"]) == 64)
        check("policy: without --lenient the run with mismatches exits 3", prun("policy1b")[0] == 3)
        served(q_interval=doc_(["v"], [[float_("1.5")]]), q_lexical=doc_(["v"], [[float_("1.5")]]), q_cutoff=doc_(["p"], [[uri_("a")], [uri_("b")], [uri_("z")]]), q_empty=doc_(["x"], []))
        rc, res, _ = prun("policy2", "--lenient"); q = res["queries"]
        check("policy: out-of-interval, a changed value and an invalid cutoff selection are mismatches; a matching empty answer passes",
              all(q[c]["class"] == "mismatch" for c in ("q_interval", "q_lexical", "q_cutoff")) and q["q_empty"]["class"] == "pass")
        served(q_interval=doc_(["v"], [[float_("1.0")]]), q_lexical=doc_(["v"], [[float_("1.0")]]), q_cutoff=ref_answers["q_cutoff"], q_order_ok=ref_answers["q_order_ok"], q_order_bad=ref_answers["q_order_bad"])
        rc, res, _ = prun("policy3")
        check("policy: exact answers stay pass and the policy is not consulted (only the empty answer is, and it passes): eligible, exit 0", rc == 0 and res and res["eligible"] and all(e["class"] == "pass" for e in res["queries"].values()) and not any(e.get("policy_reasons") for e in res["queries"].values() if e["class"] != "pass"))
        # the typed projection of an empty answer is kept on the timed repeats, not only on the checking run
        edir = tmp / "pe"; edir.mkdir(); (edir / "q_empty.rq").write_text("# case: q_empty\nSELECT * WHERE {}")
        BYCASE["q_empty_wrong"] = doc_(["y"], []); behaviors_raw["/flipempty"] = ("flipempty", 0)
        out = tmp / "flip.json"
        r = subprocess.run([sys.executable, __file__, "run", "--system", "flip", "--url", f"http://127.0.0.1:{port}/flipempty", "--queries", str(edir), "--reference", str(tmp / "refpolicy.json"), "--out", str(out), "--runs", "3", "--timeout", "30",
                            "--case-policy", str(bundle / "policy.py"), "--policy-bundle", str(bundle)], capture_output=True, text=True)
        flip = json.load(open(out))["queries"]["q_empty"] if out.exists() else {}
        check("policy: a correct empty projection on the checking run and a wrong one on the repeats is a mismatch, exit 3", r.returncode == 3 and flip.get("class") == "mismatch" and any("projected variables" in x for x in flip.get("policy_reasons", [])))
        # the projected variables are compared on every suite: the digests hash values by position, so a renamed projection would otherwise pass
        served(q_interval=doc_(["w"], [[float_("1.0")]]))
        rc, res, _ = prun("projrename", "--lenient", policy=False)
        check("a renamed projected variable with the same values is a mismatch, with or without a policy", res["queries"]["q_interval"]["class"] == "mismatch")
        rc, res, _ = prun("projrename-policy", "--lenient")
        check("a renamed projection is a mismatch before the policy is consulted (no policy reason, no relaxation)", res["queries"]["q_interval"]["class"] == "mismatch" and not any("within" in x for x in res["queries"]["q_interval"].get("policy_reasons", [])))
        served()
        novars = {"queries": {c: {k: v for k, v in rec.items() if k != "vars"} for c, rec in json.loads((tmp / "refpolicy.json").read_text())["queries"].items()}}
        (tmp / "refnovars.json").write_text(json.dumps(novars))
        rc, res, err = prun("novars-unscored", "--lenient", ref="refnovars.json", policy=False)
        check("a reference without projections still works unscored (the old comparison)", res is not None and res["scored"] is False)
        # drained repeats keep the projection too: a renamed variable of the same byte length must not pass on its byte count
        odir = tmp / "po"; odir.mkdir(); (odir / "q_one.rq").write_text("# case: q_one\nSELECT * WHERE {}")
        BYCASE["q_one"] = doc_(["v"], [[int_("7")]]); BYCASE["q_one_renamed"] = doc_(["w"], [[int_("7")]]); behaviors_raw["/flipvar"] = ("flipvar", 0)
        (tmp / "refone.json").write_text(json.dumps({"queries": {"q_one": {**fold_of([BYCASE["q_one"]]), "ordered": False}}}))
        out = tmp / "flipvar.json"
        r = subprocess.run([sys.executable, __file__, "run", "--system", "flipvar", "--url", f"http://127.0.0.1:{port}/flipvar", "--queries", str(odir), "--reference", str(tmp / "refone.json"), "--out", str(out), "--runs", "3",
                            "--timeout", "30", "--drain-rows", "1"], capture_output=True, text=True)
        fv = json.load(open(out))["queries"]["q_one"] if out.exists() else {}
        check("a drained repeat with a renamed projected variable (same byte length) is not a pass, exit 3", r.returncode == 3 and fv.get("class") != "pass" and "median_s" not in fv)
        (bundle / "__pycache__").mkdir(); (bundle / "__pycache__" / "policy.cpython-313.pyc").write_bytes(b"x")
        try: mf2.policy_identity(bundle / "policy.py", bundle, {}); refused = False
        except SystemExit as e: refused = "bytecode caches" in str(e)
        check("policy: a bundle that contains bytecode caches is refused (Python could load them)", refused)
        import shutil as _sh; _sh.rmtree(bundle / "__pycache__")
        # a case the policy judged is never timed by draining (only the bytes of a drained repeat are compared)
        served(q_interval=doc_(["v"], [[float_("1.0004")]]))
        rc, res, _ = prun("policydrain", "--lenient", "--drain-rows", "1")
        qd = res["queries"]["q_interval"]
        check("policy: a relaxed answer is not timed by draining: mismatch, no median, with the reason", qd["class"] == "mismatch" and "median_s" not in qd and any("drained" in x for x in qd.get("policy_reasons", [])))
        # identity is read from bytes, not by running the module
        (bundle / "evil_policy.py").write_text("import os\nNAME = os.environ.get('X', 'n')\nDATASET_SHA256 = []\ndef start(c): pass\ndef check(*a): pass\n")
        try: mf2.policy_identity(bundle / "evil_policy.py", bundle, {}); lit = False
        except SystemExit as e: lit = "literal" in str(e)
        check("policy: a policy whose NAME is not a literal is refused, and nothing was run to find that out", lit)
        (bundle / "evil_policy.py").unlink()
        (tmp / "crlf").mkdir(); (tmp / "crlf" / "q.rq").write_bytes(b"SELECT *\r\nWHERE {}\r\n")
        check("policy: case queries are digested as file bytes (CRLF kept), as the frozen registries do", mf2.query_file_digests(tmp / "crlf")["q"] == hashlib.sha256(b"SELECT *\r\nWHERE {}\r\n").hexdigest())
        # scored runs bind the policy: the manifest records its identity, the bundle digest covers every file, and a policy bound to other data refuses
        qmap = mf2.query_file_digests(pdir)
        pm_rec = mf2.good_receipt(datetime.datetime.now(datetime.timezone.utc), mf2.boot_id()); (mdir / "machine-check.json").write_text(json.dumps(pm_rec))
        pm = mf2.good_manifest(pm_rec); pm["machine"]["receipt_sha256"] = mf2.sha256_file(mdir / "machine-check.json")
        pm["queries_sha256"] = mf2.sha256_tree(pdir); pm["reference_sha256"] = mf2.sha256_file(tmp / "refpolicy.json"); pm["protocol"].update(runs=2, timeout_s=30, seed=0, drain_rows=200000)
        pm["systems"] = [{"name": "pscored", "version": "t", "config_sha256": "1" * 64, "url": f"http://127.0.0.1:{port}/bycase", "clear_cache_url": ""}]
        pm["datasets"][0]["sha256"] = dataset_digest
        pm["policy"] = mf2.policy_identity(bundle / "policy.py", bundle, qmap)
        (mdir / "manifest.json").write_text(json.dumps(pm))
        manifest_args = ("--manifest", str(mdir / "manifest.json"), "--scored")
        rc, res, err = prun("pscored-novars", *manifest_args, ref="refnovars.json")
        check("scored: a reference without projections (`vars`) is refused, naming the reason", rc != 0 and res is None and "reference lacks projection" in err)
        rc, res, err = prun("pscored", *manifest_args)
        check("policy, scored: a run whose manifest records the policy is scored (exact answers)", rc == 0 and res and res["scored"] is True and res["case_policy"]["bundle_sha256"] == pm["policy"]["bundle_sha256"])
        rc, res, err = prun("pscored-nopolicy", *manifest_args, policy=False)
        check("policy, scored: a run without the policy that the manifest records is refused", rc != 0 and res is None and "case policy" in err)
        pm2 = json.loads(json.dumps(pm)); pm2["policy"] = None; del pm2["policy"]; (mdir / "manifest.json").write_text(json.dumps(pm2))
        rc, res, err = prun("pscored-unrecorded", *manifest_args)
        check("policy, scored: a policy the manifest does not record is refused", rc != 0 and res is None and "case policy" in err)
        (mdir / "manifest.json").write_text(json.dumps(pm))
        (bundle / "helper.py").write_text("VALUE = 1\n")
        before = mf2.sha256_tree(bundle)
        env_plain = {k: v for k, v in os.environ.items() if k != "PYTHONDONTWRITEBYTECODE"}
        subprocess.run([sys.executable, "-c", f"import sys; sys.path.insert(0, {str(HERE_DIR)!r}); import manifest as m; m.policy_identity({str(bundle / 'policy.py')!r}, {str(bundle)!r}, {{}})"], env=env_plain, capture_output=True)
        check("policy: importing the policy under plain python writes no bytecode cache into the bundle, and its digest is unchanged", not list(bundle.rglob("__pycache__")) and mf2.sha256_tree(bundle) == before)
        (bundle / "helper.py").unlink()
        (bundle / "sidecar.json").write_text("{}")
        rc, res, err = prun("pscored-sidecar", *manifest_args)
        check("policy, scored: a file added to the bundle after the manifest changes the bundle digest and is refused", rc != 0 and res is None and "case policy" in err)
        (bundle / "sidecar.json").unlink()
        pm3 = json.loads(json.dumps(pm)); pm3["datasets"][0]["sha256"] = "d" * 64; pm3["policy"]["dataset_sha256"] = ["d" * 64]; (mdir / "manifest.json").write_text(json.dumps(pm3))
        rc, res, err = prun("pscored-otherdata", *manifest_args)
        check("policy, scored: a policy bound to different dataset digests than the run's refuses to score", rc != 0 and res is None and ("bound to dataset digests" in err or "case policy" in err))
        (mdir / "manifest.json").write_text(json.dumps(pm))
        # a timeout is ineligible
        rc, res, _ = run("slow", "slow", "--timeout", "1")
        check("a timeout: ineligible, exit 3", rc == 3 and res and res["queries"]["q1"]["class"] == "timeout")
    return ok


def cmd_selftest(a):
    global CAP
    ref = {"rows": 50, **fold_of(_body(50))}
    ok = True
    cases = [("good", "ok", 50, "pass"), ("typing", "int", 50, "value-compatible"), ("wrong", "wrong", 50, "mismatch"),
             ("short", "ok", 30, "mismatch"), ("trunc", "trunc", 50, "incomplete"), ("err", "err", 0, "error"), ("unsup", "unsup", 0, "unsupported"), ("slow", "slow", 50, "timeout"),
             ("cap", "ok", 40, "incomplete")]
    behaviors = {f"/{name}": (kind, n) for name, kind, n, _ in cases}
    behaviors["/ask"] = ("ask", 0)
    srv, port = serve_fake(behaviors)
    CAP = 40                       # the fake "cap" endpoint returns 40 of the 50 rows
    for name, kind, n, want in cases:
        rec, secs = one_run(f"http://127.0.0.1:{port}/{name}", "SELECT * WHERE {}", 1 if name == "slow" else 30)
        got = classify(rec, ref)
        print(f"{name:<8} want {want:<17} got {got:<17} {'ok' if got == want else 'FAIL'}"); ok &= got == want
    rec, _ = one_run(f"http://127.0.0.1:{port}/ask", "ASK {}", 5)
    good = rec["status"] == "ok" and rec["rows"] == 1 and rec["bag"] == fold_boolean(True)["bag"]
    print("ask      boolean answer", "ok" if good else "FAIL"); ok &= good
    # the projected variables come from the top-level head object, never from a `vars` list elsewhere in the prefix
    def check(label, good, detail=""):
        nonlocal ok
        print(f"{label:<80} {'ok' if good else 'FAIL ' + detail}"); ok &= bool(good)
    hv = [("a normal head", '{"head":{"vars":["a","b"]},"results":{"bindings":[]}}', ["a", "b"]),
          ("metadata with its own vars before the head", '{"meta":{"vars":["x"]},"head":{"vars":["y"]},"results":{"bindings":[]}}', ["y"]),
          ("a metadata string that looks like a head", '{"meta":"}{ \\"vars\\": [\\"z\\"]","head":{"vars":["a"]}}', ["a"]),
          ("a head after nested metadata", '{"meta":{"a":[{"vars":["q"]}],"b":"]}"},"head":{"link":[],"vars":["r"]}}', ["r"]),
          ("a nested vars inside head is not head.vars", '{"head":{"x":{"vars":["no"]}},"results":{}}', None),
          ("results before head has no readable head in the prefix", '{"results":{"bindings":[{"a":1}', None)]
    for label, doc, want in hv:
        check(f"head.vars: {label}", head_vars(doc) == want, str(head_vars(doc)))
    dr = drain(iter([b'{"meta":{"vars":["x"]},"head":{"vars":["y"]},"results":{"bindings":[]}}']), time.monotonic() + 5)
    check("a drained body reports the top-level head.vars, not the first vars in the prefix", dr["vars"] == ["y"] and dr["tail_ok"])
    sf = stream_fold(iter([b'{"meta":{"vars":["x"]},"head":{"vars":["y"]},"results":{"bindings":[]}}']), time.monotonic() + 5)
    check("the folded parser reports the same projection from the same document", sf["vars"] == ["y"])
    # streaming equals the plain parse on a 100k-row sample, with the document split at awkward chunk sizes
    global MIN_READ
    for rows, size in ((100_000, 1 << 20), (100_000, 4096), (2_000, 7), (2_000, 1)):
        doc = "".join(_body(rows)); plain = Fold(["s", "n"])
        for row in json.loads(doc)["results"]["bindings"]: plain.add(row)
        data = doc.encode()
        MIN_READ = 0                 # every chunk is parsed on its own, so the awkward boundaries are really tested
        try: got = stream_fold(iter([data[i:i + size] for i in range(0, len(data), size)]), time.monotonic() + 600)
        finally: MIN_READ = 64 << 10
        same = got == plain.result(); print(f"stream   {rows} rows, chunks of {size} bytes", "ok" if same else "FAIL"); ok &= same
    # large answers: a drained run returns the same byte count as the hashed check run, and a cut-off document fails the tail test
    for name in ("good", "trunc"):
        full, _ = one_run(f"http://127.0.0.1:{port}/{name}", "SELECT * WHERE {}", 30)
        dr, _ = one_run(f"http://127.0.0.1:{port}/{name}", "SELECT * WHERE {}", 30, drain_only=True)
        if name == "good": good = dr["status"] == "ok" and dr["bytes"] == full["bytes"] and dr["tail_ok"]
        else: good = full["status"] == "truncated" and dr["status"] == "ok" and not dr["tail_ok"]
        print(f"drain    {name}: bytes {dr.get('bytes')}, tail_ok {dr.get('tail_ok')}", "ok" if good else "FAIL"); ok &= good
    # a volatile `query-time-ms` trailer of changing length: the check run and every drained run agree, and the answer still classifies as pass
    behaviors["/qtime"] = ("qtime", 50)
    full, _ = one_run(f"http://127.0.0.1:{port}/qtime", "SELECT * WHERE {}", 30)
    drains = [one_run(f"http://127.0.0.1:{port}/qtime", "SELECT * WHERE {}", 30, drain_only=True)[0] for _ in range(3)]
    good = classify(full, ref) == "pass" and all(d["status"] == "ok" and d["bytes"] == full["bytes"] and d["tail_ok"] for d in drains)
    print(f"qtime    trailer of changing length: check {full.get('bytes')}, drained {[d.get('bytes') for d in drains]}", "ok" if good else "FAIL"); ok &= good
    behaviors["/count8"] = ("count=8", 0); behaviors["/count9"] = ("count=9", 0); behaviors["/slow"] = ("slow", 50)
    behaviors["/count9s"] = ("count9s", 0); behaviors["/twice"] = ("twice", 0)
    behaviors["/slowfirst"] = ("slowfirst", 0); behaviors["/asc"] = ("asc", 0); behaviors["/desc"] = ("desc", 0)
    ok &= gate_tests(port, behaviors)
    print("SELFTEST", "PASSED" if ok else "FAILED"); sys.exit(0 if ok else 1)


def fold_of(parts):
    return stream_fold(iter([p.encode() for p in parts]), time.monotonic() + 600)


def cmd_selftest_big(a):
    """5M rows through the streaming fold in one process; prints the rows folded and the peak RSS in MB."""
    import resource
    rows = a.rows
    r = stream_fold((p.encode() for p in _body(rows)), time.monotonic() + 3600)
    rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    mb = rss / (1 << 20) if sys.platform == "darwin" else rss / 1024
    print(f"rows={r['rows']} peak_rss_mb={mb:.0f}"); sys.exit(0 if r["rows"] == rows else 1)


def main():
    p = argparse.ArgumentParser(); sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("reference"); r.add_argument("--queries", required=True); r.add_argument("--url", required=True)
    r.add_argument("--out", required=True); r.add_argument("--timeout", type=float, default=300); r.set_defaults(f=cmd_reference)
    x = sub.add_parser("run"); x.add_argument("--system", required=True); x.add_argument("--url", required=True)
    x.add_argument("--queries", required=True); x.add_argument("--reference", required=True); x.add_argument("--out", required=True)
    x.add_argument("--mode", choices=["warm", "cold"], default="warm"); x.add_argument("--cold-hook")
    x.add_argument("--drain-rows", type=int, default=200000, help="answers of at least this many reference rows are timed by draining the body, after one hashed check run")
    x.add_argument("--clear-cache-url", help="GET before every request (warm-up and timed) to empty the server's result cache")
    x.add_argument("--restart-hook", help="command that restarts the server; run after every timed-out query (warm mode)")
    x.add_argument("--runs", type=int, default=5); x.add_argument("--timeout", type=float, default=300); x.add_argument("--seed", type=int, default=0)
    x.add_argument("--manifest", help="run manifest (manifest.py write); a run without a complete one, or whose machine receipt is stale, failed or from another boot, is not scored")
    x.add_argument("--case-policy", help="a Python module inside --policy-bundle that judges answers the exact comparison rejects (a registered, weaker comparison); see load_policy")
    x.add_argument("--policy-bundle", help="the directory holding the case policy, everything it imports and its sidecars; the manifest binds the digest of the whole directory")
    x.add_argument("--plan-runner", help="set by bench/eval/plan.py: the manifest's run id; the only way a cold run can be scored")
    x.add_argument("--scored", action="store_true", help="refuse to run unless the manifest and the machine receipt qualify")
    x.add_argument("--expect-unsupported", help="comma-separated case IDs this system configuration is known not to answer (QLever's two-order index and a predicate variable); only these may come back `unsupported` in an eligible run")
    x.add_argument("--lenient", action="store_true", help="exploration only: exit 0 even when the run is ineligible for a comparison. By default a wrong answer, a timeout, a failed hook or a failed cache clear exits with status 3; the results file (with `eligible: false` and the reasons) is written either way")
    x.set_defaults(f=cmd_run)
    s = sub.add_parser("selftest"); s.set_defaults(f=cmd_selftest)
    b = sub.add_parser("selftest-big"); b.add_argument("--rows", type=int, default=5_000_000); b.set_defaults(f=cmd_selftest_big)
    a = p.parse_args(); a.f(a)


if __name__ == "__main__":
    main()
