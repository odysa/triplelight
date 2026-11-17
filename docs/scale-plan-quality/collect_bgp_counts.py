#!/usr/bin/env python3
"""Read-only exact COUNT probes against an explicitly selected reference endpoint.
No queries run until invoked. Use a benchmark lock and a fixed dataset on that endpoint.
"""
import argparse, hashlib, json, pathlib, urllib.parse, urllib.request
ap = argparse.ArgumentParser()
ap.add_argument("probes", type=pathlib.Path)
ap.add_argument("endpoint")
ap.add_argument("output", type=pathlib.Path)
ap.add_argument("--dataset-id", required=True, help="Immutable dataset receipt, not just scale name")
ap.add_argument("--timeout", type=int, default=600)
a = ap.parse_args()
if a.output.exists():
    raise SystemExit("Refusing to overwrite an existing evidence directory")
a.output.mkdir()
files = sorted((a.probes / "count").glob("*.rq"))
manifest = (a.probes / "manifest.tsv").read_bytes()
expected = {line.split("\t")[0] for line in manifest.decode().splitlines()[1:]}
if not files or {p.stem for p in files} != expected:
    raise SystemExit("Manifest/probe cohort differs or is empty")
receipts, rows = [], []
for p in files:
    q = p.read_bytes()
    request = urllib.request.Request(a.endpoint, data=urllib.parse.urlencode({"query": q.decode()}).encode(),
        headers={"Accept": "application/sparql-results+json", "Content-Type": "application/x-www-form-urlencoded"})
    with urllib.request.urlopen(request, timeout=a.timeout) as response:
        body = response.read()
    a.output.joinpath(p.stem + ".response.json").write_bytes(body)
    r = json.loads(body)
    variables = r["head"]["vars"]
    bindings = r["results"]["bindings"]
    if len(variables) != 1 or len(bindings) != 1:
        raise RuntimeError(f"{p.stem}: COUNT must return exactly one scalar")
    scalar = bindings[0][variables[0]]
    if scalar["type"] not in ("literal", "typed-literal"):
        raise RuntimeError(f"{p.stem}: COUNT is not a literal")
    lexical = scalar["value"]
    if not lexical.isascii() or not lexical.isdigit():
        raise RuntimeError(f"{p.stem}: COUNT is not a nonnegative integer")
    n = int(lexical)
    if n > 2**64 - 1:
        raise OverflowError("Reference count exceeds optimizer harness u64")
    rows.append(f"{p.stem}\t{n}\n")
    receipts.append({"probe": p.stem, "query_sha256": hashlib.sha256(q).hexdigest(),
        "response_sha256": hashlib.sha256(body).hexdigest(), "rows": n})
a.output.joinpath("counts.tsv").write_text("".join(rows))
a.output.joinpath("receipt.json").write_text(json.dumps({
    "dataset_id_supplied_by_operator": a.dataset_id, "endpoint": a.endpoint,
    "manifest_sha256": hashlib.sha256(manifest).hexdigest(), "probes": receipts}, indent=2) + "\n")
