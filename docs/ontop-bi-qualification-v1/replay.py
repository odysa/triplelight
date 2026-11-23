"""Verify and replay the Ontop BSBM BI evidence without running either database."""

import argparse
from hashlib import sha256
import json
from pathlib import Path
import sys
import tarfile
from tempfile import TemporaryDirectory


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(data):
    return sha256(data).hexdigest()


def json_strict(data):
    def unique(pairs):
        result = dict(pairs)
        require(len(result) == len(pairs), "duplicate JSON key")
        return result
    return json.loads(data, object_pairs_hook=unique)


def replay(root):
    def data(name):
        return (root / name).read_bytes()

    def read(name):
        return json_strict(data(name))

    provenance = read("PROVENANCE.json")
    expected = provenance["files"]
    require({p.relative_to(root).as_posix() for p in root.rglob("*") if p.is_file()}
            == set(expected) | {"PROVENANCE.json"}, "incomplete packet")
    for name, known in expected.items():
        require(digest(data(name)) == known, f"packet digest differs: {name}")
    changed = provenance["portable_path_replacements"]

    def original_hash(name):
        return changed[name]["original"] if name in changed else expected[name]

    stages = ("catalog-v1", "catalog-v2", "census", "mapping", "attempt", "diagnostic")
    for stage in stages:
        path = f"raw/{stage}/FINAL.json"
        final = read(path)
        release = read(f"raw/{stage}/POSTFLIGHT.json")
        require(release["final_sha256"] == original_hash(path) and
                set(release["process_groups_clear"]) == set(final["stages"]) and
                all(row["group_clear"] for row in final["stages"].values()) and
                final["registration_sha256"] == original_hash(f"registrations/{stage}.json") and
                final["scored"] is False, f"scope or cleanup differs: {stage}")

    v1, v2 = (read(f"raw/{stage}/FINAL.json") for stage in stages[:2])
    require(v1["complete"] is False and v1["stop"] == "ValueError: scratch allocation stop reached"
            and v1["sampled_allocated_bytes"] >= 4 << 30, "v1 resource stop differs")
    require(v2["complete"] is False and v2["stages"]["prepare"]["exit_code"] == 0
            and b"route catalog size above cap" in data("raw/catalog-v2/catalog-stderr.txt"),
            "v2 branch-cap stop differs")
    view = read("raw/catalog-v2/PREPARED-VIEW.json")
    total = 10_102_032
    parts = view["partitions"]
    require(view["rows"] == total and view["partition_rule"] == "hash(s) % 16"
            and view["closed"] and view["read_only_reopen"]
            and not any(view[key] for key in ("invalid", "base_only", "view_only"))
            and [p["partition"] for p in parts] == list(range(16))
            and all(p["base"] == p["view"] and p["base"] >= 0 and p["base_only"] == p["view_only"] == 0 for p in parts)
            and sum(p["base"] for p in parts) == sum(p["view"] for p in parts) == total,
            "typed partition bag gate differs")

    sys.path.insert(0, str(root / "harness"))
    from generate_bi import validate_census
    from audit import Workload, audit
    import qualify_queries

    census = read("raw/census/CENSUS.json")
    census_final = read("raw/census/FINAL.json")
    mapping = read("raw/mapping/FINAL.json")
    catalog = read("raw/mapping/CATALOG.json")
    require(census_final["complete"] and mapping["complete"]
            and census["native_sha256"] == view["native_sha256"] and census["read_only"]
            and census_final["branches"] == mapping["triples_maps"] == len(catalog) == 639
            and census_final["rdf_type_branches"] == 593 and census_final["rdf_type_non_iri_rows"] == 0
            and census_final["rows"] == mapping["rows"] == total
            and digest(data("raw/census/CENSUS.json")) == census_final["census_sha256"]
            and digest(data("raw/mapping/CATALOG.json")) == mapping["catalog_sha256"]
            and data("raw/mapping/mapping.ttl") == validate_census(catalog, view, census, mapping["branch_cap"])
            and digest(data("raw/mapping/mapping.ttl")) == mapping["mapping_sha256"], "census or mapping differs")

    policy = read("policy.json")
    registry_manifest = "registry/CONTENTS.sha256.json"
    require(digest(data(registry_manifest)) == policy["registry_manifest_sha256"], "frozen Jena registry pin differs")
    originals = read(registry_manifest)
    for name, observed in expected.items():
        if not name.startswith("registry/") or name == registry_manifest:
            continue
        original = originals[name.removeprefix("registry/")]
        redaction = policy["registry_redactions"].get(name)
        require((redaction == {"original": original, "replay": observed}) if redaction else (original == observed),
                f"Jena file differs: {name}")
    value = policy["registry_redactions"]["registry/policies/value-policy-freeze.json"]
    require(value["original"] == qualify_queries.VALUE_SHA, "original BI value policy differs")
    qualify_queries.VALUE_SHA = value["replay"]
    binding = policy["registry_redactions"]["harness/WORKLOAD-SQL-DRAFT.json"]
    require(binding["original"] == policy["original_query_binding_sha256"]
            and binding["replay"] == policy["query_binding_sha256"], "binding provenance differs")
    suite = Workload(root / "registry", root / "harness/WORKLOAD-SQL-DRAFT.json",
                     policy["query_binding_sha256"], "bi10m")
    inputs = read("inputs.json")["schedule"]
    require(len(inputs) == 60 and sum(r["warmup"] for r in inputs) == 15
            and [r["file"] for r in inputs] == list(suite.items), "BI request schedule differs")
    for item in inputs:
        row = suite.items[item["file"]]
        require(all(item[k] == row[k] for k in ("query_sha256", "reference", "reference_sha256", "warmup", "template")),
                "query/reference identity differs")

    subject = {"blank_subject_rows": 0, "non_iri_subject_rows": 0, "read_only": True,
               "rows": total, "native_sha256": view["native_sha256"]}
    attempt = read("raw/attempt/FINAL.json")
    initial_journal = data("raw/attempt/query-events.tsv").decode().splitlines()
    require(read("raw/attempt/SUBJECTS.json") == subject and attempt["complete"] is False
            and read("raw/attempt/AUDIT.json")["class_counts"] == {"missing": 60}
            and read("raw/attempt/manifest.json")["mapping_sha256"] == mapping["mapping_sha256"]
            and initial_journal == ["repository-open\tbi10m", f"begin\tbi10m/{inputs[0]['file']}"]
            and attempt["stop"] == f"TimeoutError: query exceeded registered deadline: bi10m/{inputs[0]['file']}",
            "original timeout differs")

    diagnostics = read("raw/diagnostic/FINAL.json")
    reg = read("registrations/diagnostic.json")
    chosen = {}
    for item in inputs:
        chosen.setdefault(item["template"], item)
    require(reg["requests"] == list(chosen.values()) and set(chosen) == set(range(1, 9))
            and reg["per_query_seconds"] == 120 and read("raw/diagnostic/SUBJECTS.json") == subject,
            "diagnostic selection or subject gate differs")
    table = []
    for template, item in sorted(chosen.items()):
        label = f"bi10m/{item['file']}"
        entry = {"template": template, "request": item["file"]}
        for mode in ("answer", "native-sql"):
            key = f"{mode}-q{template}"
            folder = f"raw/diagnostic/{key}"
            row = diagnostics["cases"].get(key)
            if row is None:
                entry[mode] = "unvisited_or_stopped"
                continue
            journal = data(f"{folder}/query-events.tsv").decode().splitlines()
            prefix = ["repository-open\tbi10m", f"begin\t{label}"]
            require(row["label"] == label, "diagnostic label differs")
            if row["status"] == "request_timeout":
                require(journal == prefix and diagnostics["stages"][f"run-{key}"]["exit_code"] != 0,
                        "timeout evidence differs")
                entry[mode] = "timeout"
                continue
            require(len(journal) == 4 and journal[:2] == prefix and journal[-1] == "repository-closed\tbi10m",
                    "completed journal differs")
            if mode == "answer":
                checked = audit(root / folder / "answers", suite, [item], reg["record_cap"], json_strict)
                result = json.loads(json.dumps(checked["requests"][item["file"]]))
                require(not checked["unexpected_files"] and result == row["answer"] and row["status"] == result["class"],
                        f"typed answer policy replay differs for {label} ({sorted(k for k in result.keys() | row['answer'].keys() if result.get(k) != row['answer'].get(k))})")
                entry[mode] = row["status"]
                if row["status"] != "query_error":
                    output = f"{folder}/answers/{Path(item['file']).with_suffix('.json').name}"
                    entry["ontop_rows"] = len(read(output)["answer"]["results"]["bindings"])
                    _, _, reference = suite.prepared(item["file"], "single")
                    entry["jena_rows"] = len(reference["results"]["bindings"])
            else:
                require(read(f"{folder}/native-sql.json") == row and row["query_sha256"] == item["query_sha256"]
                        and row["native_sha256"] == view["native_sha256"] and row["scored"] is False,
                        "native SQL receipt differs")
                entry[mode] = row["status"]
                if row["status"] == "complete":
                    entry["native_sql_bytes"] = row["native_sql_bytes"]
                    entry["union_tokens"] = row["union_tokens"]
        table.append(entry)

    q5_query = chosen[5]["file"]
    q4_query = chosen[4]["file"]
    cast = "avg(xsd:float(xsd:string(?price)))"
    for q in (q4_query, q5_query):
        require(cast in data("registry/workloads/bi10m/queries/" + q).decode(), "price cast differs")
    bsbm = "http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/"
    rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
    price = [row for row in catalog if row["predicate"] == bsbm + "vocabulary/price"]
    require(price == [{"predicate": bsbm + "vocabulary/price", "kind": 2, "class_iri": None,
                        "datatype": bsbm + "vocabulary/USD", "lang": None, "rows": 570000}],
            "Q5 price branch differs")
    product_type = [row for row in catalog if row["predicate"] == rdf_type
                    and row["class_iri"] == bsbm + "instances/ProductType487"]
    require(len(product_type) == 1 and product_type[0]["rows"] == 30 and product_type[0]["kind"] == 0,
            "Q5 ProductType487 branch differs")
    iri_predicates = [bsbm + "vocabulary/" + name for name in ("reviewFor", "country", "product")]
    iri_predicates.append("http://purl.org/stuff/rev#reviewer")
    iri_rows = {}
    for predicate in iri_predicates:
        observed = [row for row in catalog if row["predicate"] == predicate]
        require(len(observed) == 1 and observed[0]["kind"] == 0
                and observed[0]["datatype"] is None and observed[0]["lang"] is None,
                "Q5 join predicate domain differs")
        iri_rows[predicate] = observed[0]["rows"]
    groups = suite.numeric.for_instance(suite.requests[q5_query])["groups"]
    _, _, reference = suite.prepared(q5_query, "single")
    products = {row["product"]["value"] for row in reference["results"]["bindings"]}
    require(len(groups) == 30 and len(products) == 8 and products <= groups.keys()
            and all(groups[product]["n"] > 0 for product in products)
            and table[3]["answer"] == "binary32_difference" and table[3]["ontop_rows"] == 10,
            "Q5 price anchors or Q4 witness differ")
    fairness = {"price_datatype": price[0]["datatype"], "price_rows": 570000, "price_language": None,
                "target_class_rows": 30, "products_with_numeric_anchors": 30, "jena_distinct_products": 8,
                "iri_join_predicate_rows": iri_rows, "q4_same_price_cast_policy_pass": True,
                "cause": "unresolved"}
    return {"dataset": "bi10m", "rows": total, "branches": 639, "rdf_type_branches": 593,
            "original_attempt": {"timed_out": 1, "unvisited": 59, "answers": 0},
            "diagnostic_complete": diagnostics["complete"], "bi_workload_qualified": False,
            "scored": False, "table": table, "q5_mapping_audit": fairness, "verified_members": len(expected)}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("packet", type=Path)
    args = parser.parse_args()
    path = args.packet
    require(path.with_name("PACKET.sha256").read_text().split() == [digest(path.read_bytes()), path.name],
            "archive digest differs")
    with TemporaryDirectory() as temp, tarfile.open(path, "r:gz") as archive:
        members = archive.getmembers()
        require(len(members) <= 800 and sum(m.size for m in members) <= 128 << 20
                and all(m.isfile() and not Path(m.name).is_absolute() and ".." not in Path(m.name).parts
                        for m in members)
                and len({m.name for m in members}) == len(members), "invalid archive shape")
        archive.extractall(temp, members=members, filter="data")
        print(json.dumps(replay(Path(temp)), indent=2, sort_keys=True))
