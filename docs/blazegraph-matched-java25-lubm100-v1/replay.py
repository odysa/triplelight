"""Replay the matched Java 25 LUBM-100 qualification without starting either engine."""

import argparse
from hashlib import file_digest
import json
from pathlib import Path
import sys
import tarfile
from tempfile import TemporaryDirectory

ENGINES = ("blazegraph", "triplelight")
STAGES = {
    "blazegraph/load", "blazegraph/reopen", "blazegraph/query",
    "triplelight/build", "triplelight/verify", "triplelight/compile-statistics",
    "triplelight/prepare-statistics", "triplelight/reopen", "triplelight/query",
}
ORDERS = {"spo", "pso", "pos", "osp"}


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def digest(path):
    with path.open("rb") as stream:
        return file_digest(stream, "sha256").hexdigest()


def read(path):
    return json.loads(path.read_bytes())


def compare(root):
    provenance = read(root / "PROVENANCE.json")
    files = provenance["files"]
    names = {p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file()}
    require(names == set(files) | {"PROVENANCE.json"}, "archived file set differs")
    require(all(digest(root / n) == h for n, h in files.items()), "archived digest differs")
    source, run = root / "source", root / "run"
    final = read(run / "FINAL.json")
    postflight = read(run / "POSTFLIGHT.json")
    build = read(source / "BUILD.json")
    bindings = read(source / "BINDINGS.json")
    count = bindings["expected_unique_triples"]
    require(final["qualified"] is True and final["scored"] is False
            and final["registration_sha256"] == provenance["registration_sha256"]
            and final["inputs"]["bindings_sha256"] == provenance["original_bindings_sha256"]
            and final["inputs"]["build_receipt_sha256"] == digest(source / "BUILD.json")
            and final["input_before"] == final["input_after"] == bindings["input"]
            and len({q["query_sha256"] for q in bindings["queries"]}) == 26
            and len(bindings["queries"]) == 30, "run or input identity differs")
    require(postflight["final_sha256"] == provenance["original_final_sha256"]
            and postflight["supervisor_exit"] == 0 and not postflight["scoped_container_inventory"],
            "postflight differs")

    require(final["stages"] == dict.fromkeys(STAGES, "complete"), "missing stage")
    for name in STAGES:
        folder = run / name
        require(read(folder / "status.json") == {"exit_code": 0, "stop": None}, f"bad exit: {name}")
        q, p = map(int, (folder / "before.cpu.max").read_text().split())
        require(p > 0 and q == p * 4 and int((folder / "before.memory.max").read_text()) == (12 << 30)
                and int((folder / "before.memory.swap.max").read_text()) == 0, f"resource drift: {name}")
        for when in ("before", "after"):
            events = dict(row.split() for row in (folder / f"{when}.memory.events").read_text().splitlines())
            require(events.get("oom") == events.get("oom_kill") == "0", f"OOM counter: {name}")
        observation = read(folder / "observed.json")
        markers = observation.get("markers")
        if markers:
            require(markers["java_version"] == "25.0.5.1+2-LTS" and markers["java_arch"] == "aarch64"
                    and markers["available_processors"] == "4" and "-Xmx8g" in observation["java_arguments"],
                    f"JVM identity differs: {name}")
        if name in ("blazegraph/load", "blazegraph/reopen"):
            require(markers["exact_explicit_triples"] == markers["exact_triples"] == str(count),
                    f"Blazegraph count differs: {name}")
        elif name == "triplelight/reopen":
            require(markers["exact_triples"] == str(count) and markers["stats_ready"] == "true",
                    "TripleLight count or statistics readiness differs")

    bg = read(run / "blazegraph/load/store.json")
    require(bg == read(run / "blazegraph/reopen/store.json") == read(run / "blazegraph/query/store.json"),
            "Blazegraph store changed")
    tl = read(run / "triplelight/prepare-statistics/store.json")
    require(tl == read(run / "triplelight/reopen/store.json") == read(run / "triplelight/query/store.json")
            and tl["base"] == read(run / "triplelight/verify/store.json")
            and set(tl["base"]) == ORDERS | {"MANIFEST", "dictionary.unified"},
            "TripleLight store or order set changed")
    stats = tl["statistics"]
    require(stats["generation"] == 0 and stats["base_manifest_sha256"] == tl["base"]["MANIFEST"]["sha256"]
            and stats["bytes"] == tl["metadata"]["stats-0.v1"]["bytes"], "sidecar identity differs")

    sys.path.insert(0, str(source))
    from check_answers import replay

    counts = {}
    for engine in ENGINES:
        result = replay(source / "workload", run / engine / "query/answers", source / "BINDINGS.json",
                        provenance["portable_bindings_sha256"])
        require(result == final["answers"][engine] and result["stream_complete"]
                and result["all_primary_pass"] and result["all_count_compatible_pass"],
                f"answer checks differ: {engine}")
        counts[engine] = len(result["labels"])
    return {
        "verified_archive_files": len(files), "reopened_triples": count, "exact_typed_answers": counts,
        "stage_count": len(STAGES), "triplelight_commit": build["source"]["commit"],
        "triplelight_orders": sorted(ORDERS), "triplelight_apparent_store_bytes":
        sum(item["bytes"] for group in ("base", "metadata") for item in tl[group].values()),
        "blazegraph_journal_bytes": bg["bytes"], "query_timing_collected": False,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("packet", type=Path)
    args = parser.parse_args()
    require(args.packet.with_name("PACKET.sha256").read_text().split() == [digest(args.packet), args.packet.name],
            "packet checksum differs")
    with TemporaryDirectory() as temp, tarfile.open(args.packet, "r:gz") as archive:
        members = archive.getmembers()
        require(len(members) < 350 and sum(m.size for m in members) <= (128 << 20)
                and all(m.isfile() for m in members) and len({m.name for m in members}) == len(members),
                "invalid archive shape")
        archive.extractall(temp, members=members, filter="data")
        print(json.dumps(compare(Path(temp)), indent=2, sort_keys=True))
