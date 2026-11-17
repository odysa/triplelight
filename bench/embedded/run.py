#!/usr/bin/env python3
"""One in-process backend per fresh Python process; no HTTP or server startup."""
import argparse
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path
import platform
import re
import statistics
import sys
import time

from contract import check_order, json_term, python_term, signature


def sha(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


class Ladybug:
    def __init__(self, args, compiled):
        import ladybug
        # Parameter construction is workload preparation, outside query timings.
        sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "ladybug"))
        from parameters import parameterize
        self.queries = {}
        for name, rec in compiled.items():
            if "unsupported" not in rec:
                self.queries[name] = parameterize(rec["cypher"])
        start = time.perf_counter_ns()
        self.db = ladybug.Database(str(args.database), read_only=True,
                                  buffer_pool_size=args.cache_mb * 1024**2,
                                  max_num_threads=args.threads)
        self.conn = ladybug.Connection(self.db)
        self.open_ms = (time.perf_counter_ns() - start) / 1e6
        self.ready_ms = self.open_ms
        self.version = importlib.metadata.version("ladybug")

    def execute(self, name, source):
        query, params = self.queries[name]
        return self.conn.execute(query, params)

    def materialize(self, result, rec):
        table = result.get_as_arrow()
        if table.column_names != rec["vars"]:
            raise ValueError("Native Arrow columns differ from compiled projection")
        return [column.to_pylist() for column in table.columns]

    def decode(self, columns, rec):
        rows = []
        predicate_columns = set(rec.get("predicate_columns", []))
        for values in zip(*columns):
            row = []
            for var, value in zip(rec["vars"], values):
                if value is None:
                    row.append(None)
                elif var in predicate_columns:
                    row.append(json_term({"type": "uri", "value": rec["predicate_labels"][value]}))
                else:
                    row.append(json_term(json.loads(value)))
            rows.append(tuple(row))
        return rec["vars"], rows

    def close(self):
        self.conn.close()
        self.db.close()


class Rdfdb:
    def __init__(self, args, compiled):
        import rdfdb
        self.api = rdfdb
        start = time.perf_counter_ns()
        # Require a populated, dedicated durable store. Never import while timing open.
        if not (args.database / "CURRENT").is_file():
            raise ValueError("Expected an existing durable Store directory with CURRENT")
        self.db = rdfdb.Database(str(args.database))
        self.open_ms = (time.perf_counter_ns() - start) / 1e6
        try:
            if not callable(getattr(self.db, "stats_ready", None)):
                raise RuntimeError("Public Python stats_ready() required for an explicit readiness boundary")
            deadline = time.monotonic() + args.startup_timeout
            while not self.db.stats_ready():
                require_lock(args)
                if time.monotonic() > deadline:
                    raise TimeoutError("Statistics readiness timed out")
                time.sleep(0.05)
        except BaseException:
            self.db.close()
            raise
        self.ready_ms = (time.perf_counter_ns() - start) / 1e6
        self.version = importlib.metadata.version("rdfdb")

    def execute(self, name, source):
        return self.db.query(source)

    def materialize(self, result, rec):
        if result.kind != "select":
            raise ValueError("This cohort requires SELECT solutions")
        # The public API already built owned Python rows during execute_return.
        return result.vars, result.rows

    def decode(self, native, rec):
        variables, native_rows = native
        return variables, [tuple(python_term(t, self.api) for t in row) for row in native_rows]

    def close(self):
        self.db.close()


def require_lock(args):
    owner = Path("/tmp/rdf-bench.lock/owner").read_text().strip()
    if owner != args.lock_owner:
        raise RuntimeError(f"Benchmark lock does not match this job: {owner!r}")


def run(args):
    require_lock(args)
    if args.out.exists():
        raise FileExistsError("Refusing to overwrite a previous run: " + str(args.out))
    sources = {p.stem: p.read_text() for p in sorted(args.queries.glob("*.rq"))}
    if not sources:
        raise ValueError("No query files")
    compiled = json.loads(args.compiled.read_text())
    order_specs = json.loads(args.order_spec.read_text()) if args.order_spec else {}
    reference = json.loads(args.reference.read_text()) if args.reference else {}
    for name, source in sources.items():
        rec = compiled[name]
        if rec["source_sha256"] != hashlib.sha256(source.encode()).hexdigest():
            raise ValueError("Compiled query source mismatch: " + name)
        if "unsupported" in rec:
            raise ValueError("Unsupported query must be explicitly removed from this cohort: " + name)
        if re.search(r"\bORDER\s+BY\b", source, re.I) and not order_specs.get(name):
            raise ValueError("Ordered query requires explicit projected-key order spec: " + name)
    out = {"schema": "embedded-select-v1", "system": args.system,
           "database": str(args.database.resolve()), "binding_source": args.binding_source,
           "python": sys.version, "platform": platform.platform(), "threads": args.threads,
           "requested_cache_mb": args.cache_mb, "runs": args.runs,
           "query_boundary": "public Python execute + full native materialization + semantic RDF decoding",
           "hash_boundary": "outside all query timings; sorted complete bag with variable names",
           "compiled_sha256": sha(args.compiled), "queries": {}, "complete": False}

    def checkpoint():
        args.out.parent.mkdir(parents=True, exist_ok=True)
        temp = args.out.with_suffix(args.out.suffix + ".tmp")
        temp.write_text(json.dumps(out, indent=2))
        temp.replace(args.out)

    backend = None
    try:
        require_lock(args)
        backend = (Ladybug if args.system == "ladybug" else Rdfdb)(args, compiled)
        out.update(version=backend.version, open_ms=backend.open_ms, ready_ms=backend.ready_ms,
                   rdfdb_cache_option="binding default; cache_mb is NOT applied" if args.system == "rdfdb" else None)
        checkpoint()
        for name, source in sources.items():
            rec = compiled[name]
            query = {"source_sha256": rec["source_sha256"], "samples": []}
            out["queries"][name] = query
            expected = reference.get(name)
            first = None
            for iteration in range(args.runs + 1):
                require_lock(args)
                start = time.perf_counter_ns()
                result = backend.execute(name, source)
                returned = time.perf_counter_ns()
                native = backend.materialize(result, rec)
                materialized = time.perf_counter_ns()
                variables, rows = backend.decode(native, rec)
                consumed = time.perf_counter_ns()
                sig = signature(variables, rows)
                ordered = check_order(variables, rows, order_specs[name]) if name in order_specs else {"checked": False, "reason": "no ORDER BY"}
                sample = {"warmup": iteration == 0, "execute_return_ms": (returned-start)/1e6,
                          "materialize_ms": (materialized-returned)/1e6,
                          "semantic_decode_ms": (consumed-materialized)/1e6,
                          "fully_consumed_ms": (consumed-start)/1e6,
                          "signature": sig, "order": ordered}
                if first is not None and sig != first:
                    raise ValueError(name + " changed its complete result bag across repetitions")
                first = sig
                if expected is not None and sig != expected:
                    raise ValueError(name + " differs from the reference complete bag")
                sample["matches_reference"] = None if expected is None else True
                query["samples"].append(sample)
                checkpoint()
                # Destruction, hashing and canonical sorting are outside the measured boundary.
                del result, native, rows
            query["median_fully_consumed_ms"] = statistics.median(s["fully_consumed_ms"] for s in query["samples"] if not s["warmup"])
            print(name, query["median_fully_consumed_ms"], first["rows"], flush=True)
        out["complete"] = True
        out["externally_validated"] = bool(reference) and all(name in reference for name in sources)
    except Exception as exc:
        out["error"] = f"{type(exc).__name__}: {exc}"
        raise
    finally:
        try:
            if backend is not None:
                backend.close()
        finally:
            checkpoint()


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--system", choices=["ladybug", "rdfdb"], required=True)
    for option in ("database", "queries", "compiled", "out"):
        p.add_argument("--" + option, type=Path, required=True)
    p.add_argument("--reference", type=Path, help="query name -> contract.signature output from independently validated results")
    p.add_argument("--order-spec", type=Path)
    p.add_argument("--binding-source", required=True, help="Exact binding commit / wheel source, not inferred from version")
    p.add_argument("--lock-owner", required=True, help="Exact current owner string of your acquired lock")
    p.add_argument("--runs", type=int, default=5)
    p.add_argument("--threads", type=int, default=4)
    p.add_argument("--cache-mb", type=int, default=10240)
    p.add_argument("--startup-timeout", type=float, default=600)
    args = p.parse_args()
    if min(args.runs, args.threads, args.cache_mb) < 1:
        p.error("runs, threads and cache-mb must be positive")
    os.environ["RAYON_NUM_THREADS"] = str(args.threads)
    run(args)
