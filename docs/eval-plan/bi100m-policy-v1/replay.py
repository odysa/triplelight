#!/usr/bin/env python3
"""Replay the portable BI-100M policy on the six published answer sets."""

import argparse
from collections import Counter
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
from tempfile import TemporaryDirectory

PROFILE = "30f38a23b2984443f64d4242cbdbcca0ae9cd68e"
HISTORICAL = "c35006126a48c6df516ad6fa8de6eb73da74e9ba"
ARMS = {f"pass{n}-{engine}" for n in (1, 2) for engine in ("duckdb", "qlever", "rdfdb")}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def run(repo, archive_path, bundle):
    def git(*args):
        return subprocess.check_output(["git", "-C", str(repo), *args])

    evidence = json.loads(archive_path.with_name("PATH-SCRUB.json").read_bytes())
    if sha(archive_path.read_bytes()) != evidence["archive_sha256"]:
        raise ValueError("Registry archive digest differs")
    with tarfile.open(archive_path) as archive:
        members = archive.getmembers()
        if (len(members) != evidence["files"] or any(not m.isfile() for m in members)
                or len({m.name for m in members}) != len(members)):
            raise ValueError("Unexpected policy archive inventory")
        archive.extractall(bundle, members=members, filter="data")

    sys.path.insert(0, str(repo / "bench/eval"))
    from driver import Fold

    module_path = bundle / "bi_case_policy.py"
    shutil.copyfile(repo / "bench/eval/bi_case_policy.py", module_path)
    spec = importlib.util.spec_from_file_location("bi100m_replay_policy", module_path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    manifest = json.loads((bundle / "registry/workloads/bi100m/manifest.json").read_bytes())
    queries = {
        Path(q["file"]).stem: sha(git("show", f"{PROFILE}:bench/profile/v4/queries/bi100/{q['file']}"))
        for q in manifest["queries"]
    }
    references = json.loads(git("show", f"{PROFILE}:bench/profile/v4/references/bi100.json"))["queries"]
    module.start({
        "case_queries": queries,
        "dataset_sha256": [manifest["source_data_sha256"]],
        "bundle_dir": bundle,
        "reference_records": references,
        "fold": Fold,
    })

    observed = {arm: {} for arm in ARMS}
    prefix = "docs/matched-bi100-v1/round1"
    with subprocess.Popen(
        ["git", "-C", str(repo), "archive", HISTORICAL, prefix], stdout=subprocess.PIPE,
    ) as process:
        with tarfile.open(fileobj=process.stdout, mode="r|") as archive:
            for member in archive:
                path = Path(member.name)
                if path.suffix != ".body" or path.stem not in references:
                    continue
                answer = json.load(archive.extractfile(member))
                ref = references[path.stem]
                rows, variables = answer["results"]["bindings"], answer["head"]["vars"]
                fold = Fold(variables)
                for row in rows:
                    fold.add(row)
                digest = fold.result()
                exact = digest["rows"] == ref["rows"] and digest["bag"] == ref["bag"]
                exact = exact and (not ref["ordered"] or digest["seq"] == ref["seq"])
                if exact:
                    status, reason = module._adapter.check(path.stem, answer, ref)
                else:
                    status, reason = module.check(path.stem, {"vars": variables, "rows": rows}, ref)
                if status not in {"pass", "value-compatible"}:
                    raise ValueError(f"{path.stem} in {path.parts[-3]}: {reason}")
                arm = observed[path.parts[-3]]
                if path.stem in arm:
                    raise ValueError(f"Duplicate saved answer for {path.stem}")
                arm[path.stem] = status
        if process.wait() != 0:
            raise RuntimeError("Could not read the pinned historical response tree")

    if any(arm.keys() != references.keys() for arm in observed.values()):
        raise ValueError("Saved answer coverage differs from frozen BI cases")
    return {
        "archive_sha256": evidence["archive_sha256"],
        "dataset_sha256": manifest["source_data_sha256"],
        "profile_commit": PROFILE,
        "saved_answers_commit": HISTORICAL,
        "queries": len(queries),
        "answers": sum(map(len, observed.values())),
        "arms": {arm: dict(Counter(statuses.values())) for arm, statuses in observed.items()},
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    args = parser.parse_args()
    archive_path = Path(__file__).with_name("registry.tar.gz")
    with TemporaryDirectory() as tmp:
        print(json.dumps(run(args.repo.resolve(), archive_path, Path(tmp)), indent=2, sort_keys=True))
