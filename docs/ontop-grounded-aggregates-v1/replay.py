"""Replay the untimed Ontop aggregate evidence without running either engine."""

import argparse
from hashlib import sha256
import importlib.util
import json
from pathlib import Path
import tarfile
from tempfile import TemporaryDirectory


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def read(path):
    return json.loads(path.read_bytes())


def digest(path):
    return sha256(path.read_bytes()).hexdigest()


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def replay(root):
    provenance = read(root / "PROVENANCE.json")
    files = provenance["files"]
    found = {path.relative_to(root).as_posix() for path in root.rglob("*") if path.is_file()}
    require(found == set(files) | {"PROVENANCE.json"}, "incomplete packet")
    for name, known in files.items():
        require(digest(root / name) == known, f"digest differs: {name}")
    baseline = module("column_valued_initialization", root / "initialization/replay.py")
    harness = root / "harness/ontop46-aggregate-view-gate"
    checker = module("grounded_answer_checker", harness / "check_answers.py")
    classify = module("grounded_answer_classifier", root / "harness/ontop46-aggregate-evidence-v1/classify.py")
    generator = module("ground_map_generator", root / "harness/ontop46-ground-mapping-draft/generate.py")
    bindings_path = harness / "AGGREGATE-BINDINGS.json"
    archive_path = harness / "FROZEN-QUERY-REFERENCES.json"
    binding = read(bindings_path)["queries"]
    archive = read(archive_path)
    require(archive["bindings_sha256"] == digest(bindings_path), "workload registry binding differs")
    registry = harness / archive["root"]
    summary = {}
    for dataset in ("lubm", "watdiv"):
        v1 = baseline.verify(dataset)
        positive = root / "grounded" / dataset
        catalog = root / "catalog" / dataset
        final = read(positive / "FINAL.json")
        expected = provenance["datasets"][dataset]
        require(final["registration_sha256"] == expected["query_registration_sha256"]
                and final["scored"] is False and final["dataset"] == dataset,
                "grounded receipt identity differs")
        postflight = read(positive / "POSTFLIGHT.json")
        require(postflight["final_sha256"] == expected["raw_query_final_sha256"]
                and postflight["stages_verified"] == final["stages"]
                and all(stage["group_clear"] is True for stage in final["stages"].values()),
                "grounded postflight receipt differs")
        receipts = read(catalog / "FINAL.json")
        rows, maps = expected["rows"], expected["triples_maps"]
        require(receipts["complete"] is True and receipts["triples_maps"] == maps == final["triples_maps"]
                and receipts["registration_sha256"] == expected["catalog_registration_sha256"]
                and receipts["rows"] == rows == v1["view_rows"]
                and digest(catalog / "CATALOG.json") == receipts["catalog_sha256"],
                "catalog receipt identity differs")
        view = read(root / "initialization" / dataset / "PREPARED-VIEW.json")
        previous_copy = read(root / "initialization" / dataset / "PREPARED-IDENTITY.json")
        copied = read(positive / "COPY.json")
        require(copied == {"bytes": previous_copy["identity"][2], "sha256": previous_copy["sha256"]},
                "private query copy receipt differs")
        domains = {row["kind"]: row["rows"] for row in view["domains"]}
        branches = read(catalog / "CATALOG.json")
        require(len(branches) == maps and generator.generate(branches, rows, domains) ==
                (positive / "mapping.ttl").read_bytes() == (catalog / "mapping.ttl").read_bytes()
                and digest(positive / "mapping.ttl") == receipts["mapping_sha256"], "grounded mapping differs")
        subject = read(positive / "SUBJECTS.json")
        require(subject == {"rows": rows, "blank_subject_rows": 0, "non_iri_subject_rows": 0,
                            "native_sha256": view["native_sha256"], "read_only": True},
                "subject domain receipt differs")
        for label, record in binding[dataset].items():
            names = archive["files"][dataset][label]
            require(digest(registry / names["query"]) == record["rule"]["query_sha256"]
                    and digest(registry / names["reference"]) == record["reference_sha256"],
                    f"frozen query/reference digest differs: {label}")
        audit = checker.audit(positive / "answers", bindings_path, archive_path, dataset, 64 << 20)
        if (positive / "AUDIT.json").is_file():
            require(read(positive / "AUDIT.json") == audit, "saved answer audit differs")
        if final["complete"]:
            require((final["all_exact"], final["all_count_compatible"]) ==
                    (audit["all_exact"], audit["all_count_compatible"]), "final verdict differs")
        classified = classify.classify(positive, dataset, 64 << 20)
        require(classified["scheduled"] == v1["scheduled"], "schedule differs")
        categories = {name: entry["count"] for name, entry in classified["classes"].items()}
        if (positive / "query-events.tsv").is_file():
            events = (positive / "query-events.tsv").read_text().splitlines()
            starts = [line.split("\t")[1] for line in events if line.startswith("begin\t")]
            require(starts == list(binding[dataset])[:len(starts)], "reached query order differs")
            classified["requests_started"] = len(starts)
            if final["complete"]:
                require(events[-1] == f"repository-closed\t{dataset}", "missing repository-close event")
            elif events[-1].startswith("begin\t"):
                classified["interrupted_request"] = events[-1].split("\t")[1]
        summary[dataset] = {"before_grounding": v1, "after_grounding": classified,
                            "classes": categories, "maps": maps, "rows": rows,
                            "exact": categories.get("exact", 0),
                            "count_compatible": sum(x.get("count_compatible_pass") is True
                                                    for x in audit["labels"].values())}
    interruption = root / "watdiv-console-stop"
    console_stop = None
    if interruption.is_dir():
        old = read(interruption / "FINAL.json")
        partial = checker.audit(interruption / "answers", bindings_path, archive_path, "watdiv", 64 << 20)
        completed = [label for label, r in partial["labels"].items() if r.get("primary_pass") is True]
        logged = (interruption / "query-events.tsv").read_text().splitlines()
        require(old["complete"] is False and old["stop"] == "ValueError: log cap reached"
                and len(completed) == 4 and logged[-1] == "begin\twatdiv/all_F2.rq"
                and read(interruption / "POSTFLIGHT.json")["stages_verified"] == old["stages"],
                "prior WatDiv console stop differs")
        console_stop = {"exact_before_stop": 4, "interrupted_request": "watdiv/all_F2.rq"}
    return {"datasets": summary, "archive_files_verified": len(files), "scored": False,
            "prior_watdiv_console_stop": console_stop}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("packet", type=Path)
    args = parser.parse_args()
    known = args.packet.with_name("PACKET.sha256").read_text().split()
    require(known == [digest(args.packet), args.packet.name], "archive digest differs")
    with TemporaryDirectory() as temporary, tarfile.open(args.packet, "r:gz") as archive:
        members = archive.getmembers()
        require(len(members) <= 320 and sum(member.size for member in members) <= 128 << 20
                and all(member.isfile() for member in members)
                and len({member.name for member in members}) == len(members), "invalid archive shape")
        archive.extractall(temporary, members=members, filter="data")
        print(json.dumps(replay(Path(temporary)), indent=2, sort_keys=True))
