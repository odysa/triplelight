#!/usr/bin/env python3
"""Replay the diagnostic from typed RDF4J records and archived Jena terms."""
import argparse
import hashlib
from collections import Counter
import json
from pathlib import Path
import re
import tarfile
from tempfile import TemporaryDirectory

XSD = "http://www.w3.org/2001/XMLSchema#"
RDF_LANG = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString"
CASES = ("plain", "distinct", "group", "count", "distinct_count")
PLAN_CASES = ("distinct", "distinct_count")
DELTAS = {
    "original": ("urn:iri", "1"),
    "split-text": ("urn:split-text", "1"),
    "split-number": ("urn:iri", "2"),
    "split-both": ("urn:split-text", "2"),
}
PATTERNS = {
    (9, 9, 9, 9): "exact_rdf_identity",
    (8, 9, 8, 9): "lexical_collapse",
    (8, 8, 9, 9): "numeric_collapse",
    (7, 8, 8, 9): "both_collapses",
}


def read(path):
    return json.loads(path.read_text(encoding="utf-8"))


def term_key(term):
    kind, value = term["type"], term["value"]
    if not isinstance(value, str):
        raise ValueError("RDF term value must be text")
    if kind == "uri" and set(term) == {"type", "value"}:
        return kind, value, None, None
    if kind != "literal" or set(term) - {"type", "value", "datatype", "xml:lang"}:
        raise ValueError("unexpected RDF term shape")
    language = term.get("xml:lang")
    datatype = term.get("datatype", RDF_LANG if language else XSD + "string")
    if not isinstance(datatype, str) or (language is not None and (not isinstance(language, str) or not language or datatype != RDF_LANG)):
        raise ValueError("unexpected literal datatype")
    return kind, value, datatype, language.lower() if language else None


def typed_integer(number):
    return "literal", str(number), XSD + "integer", None


def answer_bag(answer, variables):
    if answer["head"]["vars"] != list(variables):
        raise ValueError("unexpected projection")
    rows = answer["results"]["bindings"]
    if not isinstance(rows, list) or any(set(row) != set(variables) for row in rows):
        raise ValueError("unexpected result row shape")
    return Counter(tuple(term_key(row[v]) for v in variables) for row in rows)


def expected(source, text, number):
    ref = read(source / "consumer-fixture-jena.json")
    groups = answer_bag(ref, ("o", "n"))
    if len(groups) != 9 or any(n != typed_integer(1) or count != 1 for (_, n), count in groups.items()):
        raise ValueError("archived reference differs from the nine RDF singleton groups")
    changes = {
        ("literal", "urn:iri", XSD + "string", None): text,
        ("literal", "1", XSD + "integer", None): number,
    }
    if not changes.keys() <= {o for o, _ in groups}:
        raise ValueError("missing terms for the planned fixture deltas")
    terms = [(kind, changes.get((kind, lex, dt, lang), lex), dt, lang) for (kind, lex, dt, lang), _ in groups]
    if len(set(terms)) != 9:
        raise ValueError("fixture delta changed the expected term cardinality")
    return {
        "plain": (("o",), Counter((term,) for term in terms)),
        "distinct": (("o",), Counter((term,) for term in terms)),
        "group": (("o", "n"), Counter((term, typed_integer(1)) for term in terms)),
        "count": (("n",), Counter({(typed_integer(9),): 1})),
        "distinct_count": (("n",), Counter({(typed_integer(9),): 1})),
    }


def evaluate_profile(source, child, profile, text, number):
    queries = {case: (source / "queries" / (case + ".rq")).read_text(encoding="utf-8") for case in CASES}
    source_ok = completed = False
    try:
        jdbc = read(child / "jdbc-source.json")
        source_ok = (jdbc["profile"], jdbc["source_rows"], jdbc["id_2_lex"], jdbc["id_5_lex"],
                     jdbc["duckdb_version"], jdbc["java"]) == (profile, 11, text, number, "v1.5.6", "25.0.5.1+2-LTS")
        result = read(child / "completed.json")
        completed = result == {"profile": profile, "cases_attempted": len(CASES), "scored": False, "full_workload_qualified": False}
    except (OSError, ValueError, KeyError, TypeError):
        pass

    checks = {}
    for case, (variables, wanted) in expected(source, text, number).items():
        try:
            record = read(child / (case + ".json"))
            if record["query"] != queries[case] or "error" in record or "error_class" in record:
                raise ValueError(f"query error or identity mismatch: {record.get('error', '')}")
            actual = answer_bag(record["answer"], variables)
            checks[case] = {"matches_reference": actual == wanted}
            if case == "distinct_count" and sum(actual.values()) == 1:
                ((kind, value, datatype, lang),), = actual
                if kind == "literal" and datatype == XSD + "integer" and not lang and re.fullmatch(r"[+-]?[0-9]+", value):
                    observed = int(value)
                    if observed >= 0:
                        checks[case]["observed_integer"] = observed
        except (OSError, ValueError, KeyError, TypeError) as error:
            checks[case] = {"matches_reference": False, "error": str(error)}

    plans = {}
    for case in PLAN_CASES:
        try:
            record = read(child / (case + "-plan.json"))
            if record["query"] != queries[case]:
                raise ValueError("plan query identity mismatch")
            plans[case] = {key: {"available": isinstance(record.get(key), str) and bool(record[key].strip()),
                                 "error": record.get(key + "_error")}
                           for key in ("iq", "native_query")}
        except (OSError, ValueError, KeyError, TypeError) as error:
            plans[case] = {"error": str(error)}
    controls_pass = source_ok and completed and all(checks[c]["matches_reference"] for c in CASES[:-1])
    return {"source_rows_verified": source_ok, "driver_completed": completed, "controls_pass": controls_pass,
            "typed_answers": checks, "reformulation_availability": plans}


def evaluate(source, attempt):
    profiles = {profile: evaluate_profile(source, attempt / profile / "child", profile, *delta)
                for profile, delta in DELTAS.items()}
    counts = tuple(result["typed_answers"]["distinct_count"].get("observed_integer") for result in profiles.values())
    controls_pass = all(result["controls_pass"] for result in profiles.values())
    return {"profiles": profiles, "profile_order": list(DELTAS), "distinct_count_vector": counts,
            "all_controls_pass": controls_pass, "matching_fixture_model": PATTERNS.get(counts) if controls_pass else None,
            "full_workload_qualified": False, "scored": False}


def replay(directory):
    manifest = read(directory / "PACKET_FILES.json")
    bundle = directory / "packet.tar.gz"
    packet_sha256 = hashlib.sha256(bundle.read_bytes()).hexdigest()
    with TemporaryDirectory() as temporary, tarfile.open(bundle) as archive:
        root = Path(temporary)
        members = archive.getmembers()
        if len(members) != len(manifest) or {item.name for item in members} != set(manifest):
            raise ValueError("unexpected packet inventory")
        for item in members:
            name = Path(item.name)
            if not item.isfile() or name.is_absolute() or ".." in name.parts or item.size >= 1_000_000:
                raise ValueError("unexpected packet member")
            with archive.extractfile(item) as stream:
                data = stream.read()
            if hashlib.sha256(data).hexdigest() != manifest[item.name]:
                raise ValueError(f"packet member digest differs: {item.name}")
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open("xb") as stream:
                stream.write(data)
        actual = evaluate(root / "source", root / "attempt")
        if json.loads(json.dumps(actual)) != read(root / "attempt/diagnostic.json"):
            raise ValueError("replayed diagnostic differs from its receipt")
        if not actual["all_controls_pass"] or actual["distinct_count_vector"] != (8, 9, 8, 9):
            raise ValueError("unexpected diagnostic control outcomes")
        success = read(root / "attempt/supervisor.json")
        failure = read(root / "setup-stop/supervisor.json")
        if not success["completed"] or failure["completed"] or failure["compile"]["exit_code"] != 0:
            raise ValueError("unexpected attempt dispositions")
        plans = {case: set() for case in PLAN_CASES}
        for profile in DELTAS:
            old_log = root / "setup-stop" / profile / "stderr.txt"
            if failure["profiles"][profile]["exit_code"] != 1 or "File too large" not in old_log.read_text():
                raise ValueError("setup stop receipt differs")
            child = root / "attempt" / profile / "child"
            if read(child / "native-library.json") != success["native_asset"]:
                raise ValueError("native library receipt differs")
            for case in PLAN_CASES:
                plan = read(child / (case + "-plan.json"))
                plans[case].add(plan["native_query"])
        if any(len(sql) != 1 for sql in plans.values()):
            raise ValueError("native SQL changed across profiles")
        aggregate, = plans["distinct_count"]
        projection, = plans["distinct"]
        if not aggregate.startswith('SELECT COUNT(DISTINCT("v17"."lex16m1"))') or '"v6"' in aggregate:
            raise ValueError("aggregate no longer counts only the lexical field")
        if not projection.startswith('SELECT "v33"."lex16m1" AS "lex16m1", "v33"."v6" AS "v6"'):
            raise ValueError("DISTINCT projection no longer carries the tag")
        screen = read(root / "WORKLOAD-SCREEN.json")
        candidates = [name for name, value in screen["aggregates"].items() if value["needs_domain_evidence"]]
        if len(screen["aggregates"]) != 72 or len(screen["bi"]) != 60 or candidates != ["watdiv/distinct_L2.rq"]:
            raise ValueError("workload screen inventory differs")
        return {
            "packet_sha256": packet_sha256,
            "members": len(manifest),
            "distinct_count_vector": actual["distinct_count_vector"],
            "all_other_answer_controls_pass": True,
            "same_native_sql_across_profiles": True,
            "native_aggregate_key": "lexical field alone",
            "original_setup_stop_reproduced_by_receipts": True,
            "workload_screen_candidates": candidates,
            "full_workload_qualified": False,
            "scored": False,
        }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", nargs="?", type=Path, default=Path(__file__).resolve().parent)
    args = parser.parse_args()
    print(json.dumps(replay(args.directory), indent=2))
