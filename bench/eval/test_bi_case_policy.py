"""Replay the adapter on the extracted BI-10M packet from PR #181.

Run python3 -B bench/eval/test_bi_case_policy.py PACKET_DIRECTORY.
Extract bi10m/packet.tar.gz under docs/index-layout-watdiv-bi-laptop-v1.
"""

from collections import Counter
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import struct
import sys

from bi_case_policy import BICasePolicy
from driver import Fold


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def read(path):
    return json.loads(path.read_bytes())


def replay(packet):
    registry = packet / "registry"
    workload = registry / "workloads/bi10m"
    manifest = read(workload / "manifest.json")
    hashes = {
        Path(query["file"]).stem: sha256(workload / "queries" / query["file"])
        for query in manifest["queries"]
    }
    args = (
        registry, "bi10m", sha256(registry / "policies/value-policy-freeze.json"),
        manifest["source_data_sha256"], hashes,
    )
    adapter = BICasePolicy(*args)
    expected = {
        "Typed answer matches the frozen BI reference": 24,
        "Computed value satisfies the frozen per-entity rounding interval": 16,
        "Different xsd:float spelling at the same binary32 value": 17,
        "Selection satisfies the frozen Q7 offer-count cutoff and vendor filter": 3,
    }
    for arm in ("single", "always-spo", "longest-constant-prefix"):
        results = packet / "raw/data/queries" / arm / "results"
        reasons = Counter()
        for line in (results / "queries.jsonl").read_text().splitlines():
            record = json.loads(line)
            if record["status"] != "collected":
                continue
            case = Path(record["label"]).stem
            reference = adapter.references[case]
            answer = read(results / record["answer_file"])
            status, reason = adapter.check(case, answer, reference)
            wanted = "pass" if record["exact_typed_bag"] else "value-compatible"
            assert status == wanted, (arm, case, status, reason)
            reasons[reason] += 1
        assert reasons == expected, (arm, reasons)
        print(f"{arm}: 60 answer classes match")

    different_query = {**hashes, "m000_p04_q4": "0" * 64}
    for index, wrong in ((2, "0" * 64), (3, "0" * 64), (4, different_query)):
        changed = list(args)
        changed[index] = wrong
        try:
            BICasePolicy(*changed)
        except ValueError:
            pass
        else:
            raise AssertionError(f"Accepted a changed argument at position {index}")

    case = "m000_p04_q4"
    reference = adapter.references[case]
    for column, value in (("priceRatio", "999999"), ("feature", "urn:wrong"), ("priceRatio", "NaN")):
        answer = deepcopy(reference)
        answer["results"]["bindings"][0][column]["value"] = value
        assert adapter.check(case, answer, reference)[0] == "mismatch"
    assert adapter.check(case, reference, {})[0] == "mismatch"

    case = "m000_p12_q8"
    reference = adapter.references[case]
    answer = deepcopy(reference)
    cell = answer["results"]["bindings"][0]["cheapExpensiveRatio"]
    original = struct.unpack(">I", struct.pack(">f", float(cell["value"])))[0]
    for steps, wanted in ((4, "value-compatible"), (5, "mismatch")):
        value = struct.unpack(">f", struct.pack(">I", original + steps))[0]
        cell["value"] = repr(value)
        assert adapter.check(case, answer, reference)[0] == wanted

    case = "m000_p00_q1"
    reference = adapter.references[case]
    answer = deepcopy(reference)
    cell = answer["results"]["bindings"][0]["reviewCount"]
    cell["value"] = "+" + cell["value"]
    status, reason = adapter.check(case, answer, reference)
    assert status == "value-compatible" and "COUNT" in reason

    case = "m001_p11_q7"
    reference = adapter.references[case]
    answer = deepcopy(reference)
    answer["results"]["bindings"].append(answer["results"]["bindings"][0])
    assert adapter.check(case, answer, reference)[0] == "mismatch"
    records = {}
    for case, reference in adapter.references.items():
        folded = Fold(reference["head"]["vars"])
        for row in reference["results"]["bindings"]:
            folded.add(row)
        records[case] = folded.result()
    wrong = deepcopy(records)
    wrong[case]["bag"] = "0" * 32
    try:
        adapter.bind_references(wrong, Fold)
    except ValueError:
        pass
    else:
        raise AssertionError("Accepted a different reference digest at startup")
    adapter.bind_references(records, Fold)
    for case, reference in adapter.references.items():
        assert adapter.check(case, reference, records[case])[0] == "pass"
    assert adapter.check(case, reference, {})[0] == "mismatch"
    print("Three identity controls, eight answer/reference controls and 60 folded references pass")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    replay(Path(sys.argv[1]))
