#!/usr/bin/env python3
"""Serialized ABBA comparison; caller owns /tmp/rdf-bench.lock before invoking."""
import csv, hashlib, json, pathlib, subprocess, sys, time
base, new, data_root, query_root, output = map(pathlib.Path, sys.argv[1:])
if output.exists():
    raise SystemExit("Evidence output must be new")
output.mkdir()
def sha(p):
    with p.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()
def receipt(root):
    return {p.name: {"bytes": p.stat().st_size, "mtime_ns": p.stat().st_mtime_ns, "sha256": sha(p)}
            for p in sorted(root.iterdir()) if p.is_file()}
meta = {"order": ["base", "new", "new", "base"], "binaries": {
    "base": {"path": str(base), "sha256": sha(base)},
    "new": {"path": str(new), "sha256": sha(new)}}, "datasets": {}}
for label, qdir in [("lubm100", "lubm"), ("watdiv100", "watdiv")]:
    snapshot = data_root / ("rdfdb-disk-" + label)
    before = receipt(snapshot)
    queries = receipt(query_root / qdir)
    meta["datasets"][label] = {"snapshot": str(snapshot), "before": before, "queries": queries}
    signature = None
    for i, which in enumerate(meta["order"]):
        binary = base if which == "base" else new
        out = output / f"{label}.{i}.{which}.tsv"
        with out.with_suffix(".stderr").open("w") as err:
            subprocess.run([str(binary), str(snapshot), str(query_root/qdir), str(out)],
                           check=True, stderr=err)
        rows = list(csv.DictReader(out.open(), delimiter="\t"))
        if len(rows) != 3:
            raise RuntimeError("Missing statistics-build samples")
        current = pathlib.Path(str(out)+".plans.tsv").read_bytes()
        if signature is None:
            signature = current
        elif signature != current:
            raise RuntimeError(f"{label}: estimate bits or plan signatures differ")
        print(label, i, which, [round(int(r["nanoseconds"])/1e6, 2) for r in rows], flush=True)
    after = receipt(snapshot)
    meta["datasets"][label]["after"] = after
    if before != after or queries != receipt(query_root / qdir):
        raise RuntimeError("Input file content or metadata changed during comparison")
    meta["datasets"][label]["plans_sha256"] = hashlib.sha256(signature).hexdigest()
if sha(base) != meta["binaries"]["base"]["sha256"] or sha(new) != meta["binaries"]["new"]["sha256"]:
    raise RuntimeError("Binary changed during comparison")
(output/"receipt.json").write_text(json.dumps(meta, indent=2)+"\n")
