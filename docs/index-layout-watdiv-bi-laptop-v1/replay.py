"""Replay one DuckDB layout qualification from saved evidence and typed answers."""

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
    dataset = provenance["dataset"]
    require(final["dataset"] == dataset and final["scored"] is False and "stop" not in final
            and final["registration_sha256"] == provenance["registration_sha256"]
            and final["runtime"] == ["3.11.18", "2.0.0", "26.0.1"]
            and final["image"] == "linux/arm64 " + provenance["image"]["config"],
            "final runtime or qualification differs")
    require(set(final["stages"]) == STAGES and set(final["arms"]) == set(ARMS),
            "stage or arm inventory differs")
    for name, stage in final["stages"].items():
        expected = int(not final["arms"][name]["eligible"]) if name in ARMS else 0
        require(stage["exit_code"] == expected and stage["stop"] is None
                and stage["cleanup_verified"] is True, f"stage exit or cleanup differs: {name}")
    require(final["qualified"] == all(arm["eligible"] for arm in final["arms"].values()),
            "qualification summary differs")
    require(final["source_before"] == final["source_after"] == final["source_final"]
            and postflight["qualification_final_sha256"] == digest(raw / "FINAL.json")
            and postflight["reopen_receipt_sha256"] == digest(reopen_path)
            and postflight["reported_query_timing"] is False
            and postflight["actual_container_scope_empty_after_exit"] is True
            and postflight["dataset"] == dataset and postflight["slot_free_bytes_preclaim"] >= 64 << 30,
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
    dataset = provenance["dataset"]
    substitutions = provenance.get("redactions", {})
    allowed = {
        "registry/workloads/bi10m/manifest.json", "registry/policies/value-policy-freeze.json",
        "harness/WORKLOAD-SQL-DRAFT.json",
        *(f"registry/policies/numeric/{name}" for name in (
            "plan.json", "count-verification.json", "freeze.json", "jena-process.json",
            "jena-server.log", "validator-tests-failed-mock-metadata.log")),
    }
    require(set(substitutions) == (allowed if dataset == "bi10m" else set()), "unexpected redaction scope")
    for name, value in expected.items():
        if name.startswith("registry/") and name != "registry/CONTENTS.sha256.json":
            original = source_hashes[name.removeprefix("registry/")]
            if name in substitutions:
                require(substitutions[name] == {"original": original, "replay": value},
                        f"redacted registry entry differs: {name}")
            else:
                require(original == value, f"registry entry differs: {name}")
    final, sizes = verify_receipts(root, provenance)
    sys.path.insert(0, str(root / "harness"))
    import qualify_queries

    if dataset == "bi10m":
        binding = substitutions["harness/WORKLOAD-SQL-DRAFT.json"]
        require(binding["original"] == provenance["original_query_binding_sha256"]
                and binding["replay"] == provenance["query_binding_sha256"], "SQL binding redaction differs")
        value_policy = substitutions["registry/policies/value-policy-freeze.json"]
        require(value_policy["original"] == qualify_queries.VALUE_SHA, "original value gate identity differs")
        qualify_queries.VALUE_SHA = value_policy["replay"]
    workload = qualify_queries.Workload(
        registry, root / "harness/WORKLOAD-SQL-DRAFT.json", provenance["query_binding_sha256"], dataset)
    counts = {}
    for arm in ARMS:
        folder = root / "raw/data/queries" / arm / "results"
        schedule = {"arm": arm, "complete": False, "dataset": dataset, "eligible": False,
                    "scheduled": list(workload.items), "scored": False}
        summary = {"collected": 0, "errors": [], "exact_typed": 0}
        if dataset == "bi10m":
            summary.update(primary=0, value_ranking=0)
        else:
            summary.update(primary=0, count_compatible=0)
        require(read(folder / "SCHEDULE.json") == schedule, f"schedule differs: {arm}")
        records = [json.loads(line) for line in (folder / "queries.jsonl").read_text().splitlines()]
        require(len(records) == 2 * len(workload.items), f"journal length differs: {arm}")
        admitted = True
        for index, label in enumerate(workload.items, 1):
            first, collected = records[2 * index - 2:2 * index]
            identity = {"index": index, "label": label}
            if dataset == "bi10m":
                identity["warmup_position"] = workload.requests[label]["warmup"]
            require(first == {**identity, "status": "started"}
                    and all(collected.get(key) == val for key, val in identity.items()),
                    f"journal identity differs: {arm}/{label}")
            name = f"{index:03d}.json"
            answer = folder / name
            if collected["status"] == "error":
                require(isinstance(collected.get("error"), str) and bool(collected["error"]),
                        f"missing failure reason: {arm}/{label}")
                summary["errors"].append(label)
                admitted = False
                if "answer_file" in collected:
                    require(collected["answer_file"] == name and collected["answer_sha256"] == digest(answer),
                            f"saved partial answer identity differs: {arm}/{label}")
                continue
            require(collected["status"] == "collected" and collected["answer_file"] == name
                    and collected["answer_sha256"] == digest(answer), f"answer receipt differs: {arm}/{label}")
            _, _, reference = workload.prepared(label, arm)
            actual = workload.check(label, read(answer), reference)
            serialized = json.loads(json.dumps(actual))
            require(all(collected.get(key) == value for key, value in serialized.items()),
                    f"answer replay differs: {arm}/{label}")
            admitted = admitted and workload.admitted(actual)
            summary["collected"] += 1
            summary["exact_typed"] += int(actual["exact_typed_bag"])
            if dataset == "bi10m":
                summary["primary"] += int(actual["primary"]["status"] == "pass")
                summary["value_ranking"] += int(actual["value"]["status"] == "pass")
            else:
                summary["primary"] += int(actual["primary_pass"])
                summary["count_compatible"] += int(actual["count_compatible_pass"])
        require(read(folder / "FINAL.json") == final["arms"][arm] ==
                dict(schedule, complete=True, eligible=admitted, finished=len(workload.items)),
                f"arm receipt differs: {arm}")
        counts[arm] = summary
    return {"dataset": dataset, "orders": sorted(ORDERS), "rows_per_order": provenance["source"]["triples"],
            "db_bytes": sizes, "answers_by_arm": counts, "qualified": final["qualified"], "scored": False,
            "query_timing_collected": False, "archive_files_verified": len(expected)}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("packet", type=Path)
    args = parser.parse_args()
    recorded = args.packet.with_name("PACKET.sha256").read_text().split()
    require(recorded == [digest(args.packet), args.packet.name], "archive digest differs")
    with TemporaryDirectory() as temp, tarfile.open(args.packet, "r:gz") as archive:
        members = archive.getmembers()
        require(len(members) <= 700 and sum(m.size for m in members) <= (192 << 20)
                and all(m.isfile() for m in members)
                and len({m.name for m in members}) == len(members), "invalid archive shape")
        archive.extractall(temp, members=members, filter="data")
        print(json.dumps(replay(Path(temp)), indent=2, sort_keys=True))
