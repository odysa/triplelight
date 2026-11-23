"""Replay the LUBM-100 DuckDB qualification from saved evidence and typed answers."""

import argparse
from hashlib import file_digest
import json
from pathlib import Path
import sys
import tarfile
from tempfile import TemporaryDirectory

ARMS = ("single", "always-spo", "longest-constant-prefix")
ORDERS = {"spo", "sop", "pso", "pos", "osp", "ops"}
STAGES = {"setup", "import", "build", "reopen", *ARMS}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    with path.open("rb") as stream:
        return file_digest(stream, "sha256").hexdigest()


def read(path):
    return json.loads(path.read_bytes())


def verify_receipts(root, provenance):
    raw = root / "raw"
    final = read(raw / "FINAL.json")
    postflight = read(raw / "POSTFLIGHT.json")
    imported = read(raw / "data/control/import.json")
    built = read(raw / "data/layout/candidate/receipt.json")
    reopen_path = raw / "data/verification/reopen/receipt.json"
    reopened = read(reopen_path)
    prior = read(root / "setup-attempt-v1/FINAL.json")
    require(prior["qualified"] is False and prior["registration_sha256"] == provenance["prior_registration_sha256"]
            and set(prior["stages"]) == {"setup"} and prior["stages"]["setup"]["exit_code"] == 1,
            "initial setup stop differs")
    require(final["dataset"] == "lubm" and final["qualified"] is True and final["scored"] is False
            and final["registration_sha256"] == provenance["registration_sha256"]
            and final["runtime"] == ["3.11.18", "2.0.0", "26.0.1"]
            and final["image"] == "linux/arm64 " + provenance["image"]["config"],
            "final runtime or qualification differs")
    require(set(final["stages"]) == STAGES
            and all(s["exit_code"] == 0 and s["stop"] is None and s["cleanup_verified"] is True
                    for s in final["stages"].values()), "stage exit or cleanup differs")
    require(final["source_before"] == final["source_after"] == final["source_final"]
            and postflight["qualification_final_sha256"] == digest(raw / "FINAL.json")
            and postflight["reopen_receipt_sha256"] == digest(reopen_path)
            and postflight["reported_query_timing"] is False,
            "postflight or source identity differs")
    source = provenance["source"]
    count = source["triples"]
    require(imported["complete"] is True and imported["source_sha256"] == source["sha256"]
            and imported["source_bytes_read"] == source["bytes"] and imported["unique_triples"] == count,
            "import identity differs")
    for phase in (built, reopened):
        require(phase["complete"] is True and phase["version"] == "2.0.0"
                and set(phase["orders"]) == ORDERS, "incomplete six-copy receipt")
        for order in phase["orders"].values():
            require(order["schema_matches"] is True and order["rows"] == count
                    and order["bag_differences"] == [0, 0] and order["out_of_order_adjacent_pairs"] == 0,
                    "schema, count, bag or order differs")
    files = reopened["files_before"]
    require(files == reopened["files_after"]
            and files["control"]["bytes"] == built["control_bytes"] == imported["database_bytes"]
            and files["candidate"]["bytes"] == built["candidate_bytes"], "database identity differs")
    return final, {name: file["bytes"] for name, file in files.items()}


def replay(root):
    provenance = read(root / "PROVENANCE.json")
    expected = provenance["files"]
    observed = {str(p.relative_to(root)) for p in root.rglob("*") if p.is_file()}
    require(observed == set(expected) | {"PROVENANCE.json"}, "packet file set differs")
    for name, value in expected.items():
        require(digest(root / name) == value, f"digest differs: {name}")
    registry = root / "registry"
    manifest = registry / "CONTENTS.sha256.json"
    require(digest(manifest) == provenance["registry_manifest_sha256"], "Jena registry digest differs")
    source_hashes = read(manifest)
    for name, value in expected.items():
        if name.startswith("registry/") and name != "registry/CONTENTS.sha256.json":
            require(source_hashes[name.removeprefix("registry/")] == value, f"registry entry differs: {name}")
    final, sizes = verify_receipts(root, provenance)
    sys.path.insert(0, str(root / "harness"))
    from qualify_queries import Workload

    workload = Workload(registry, root / "harness/WORKLOAD-SQL-DRAFT.json",
                        provenance["query_binding_sha256"], "lubm")
    answer_counts = {}
    for arm in ARMS:
        folder = root / "raw/data/queries" / arm / "results"
        schedule = {"arm": arm, "complete": False, "dataset": "lubm", "eligible": False,
                    "scheduled": list(workload.items), "scored": False}
        require(read(folder / "SCHEDULE.json") == schedule, f"schedule differs: {arm}")
        require(read(folder / "FINAL.json") == final["arms"][arm]
                == dict(schedule, complete=True, eligible=True, finished=30), f"arm receipt differs: {arm}")
        records = [json.loads(line) for line in (folder / "queries.jsonl").read_text().splitlines()]
        require(len(records) == 2 * len(workload.items), f"journal length differs: {arm}")
        for index, label in enumerate(workload.items, 1):
            first, collected = records[2 * index - 2:2 * index]
            name = f"{index:03d}.json"
            answer = folder / name
            require(first == {"index": index, "label": label, "status": "started"}
                    and collected["index"] == index and collected["label"] == label
                    and collected["status"] == "collected" and collected["answer_file"] == name
                    and collected["answer_sha256"] == digest(answer), f"answer receipt differs: {arm}/{label}")
            _, _, reference = workload.prepared(label, arm)
            actual = workload.check(label, read(answer), reference)
            require(all(actual.get(key) is True for key in ("exact_typed_bag", "primary_pass", "count_compatible_pass"))
                    and all(collected.get(key) == value for key, value in actual.items()),
                    f"typed answer differs: {arm}/{label}")
        answer_counts[arm] = len(workload.items)
    return {"dataset": "lubm", "orders": sorted(ORDERS), "rows_per_order": provenance["source"]["triples"],
            "db_bytes": sizes, "exact_typed_answers": answer_counts, "scored": False,
            "query_timing_collected": False, "archive_files_verified": len(expected)}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("packet", type=Path)
    args = parser.parse_args()
    recorded = args.packet.with_name("PACKET.sha256").read_text().split()
    require(recorded == [digest(args.packet), args.packet.name], "archive digest differs")
    with TemporaryDirectory() as temp, tarfile.open(args.packet, "r:gz") as archive:
        members = archive.getmembers()
        require(len(members) <= 300 and sum(m.size for m in members) <= (128 << 20)
                and all(m.isfile() for m in members)
                and len({m.name for m in members}) == len(members), "invalid archive shape")
        archive.extractall(temp, members=members, filter="data")
        print(json.dumps(replay(Path(temp)), indent=2, sort_keys=True))
