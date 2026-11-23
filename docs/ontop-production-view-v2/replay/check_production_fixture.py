"""Replay the production-schema fixture using typed RDF4J records."""

import argparse
from collections import Counter
from hashlib import sha256
import json
from pathlib import Path
import sys

# The later fixture registration must bind this retained typed-answer validator.
sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "ontop46-distinct-diagnostic-v3"))
from check_profiles import answer_bag, term_key, typed_integer  # noqa: E402

FIXTURE = Path(__file__).resolve().parent / "tiny-production-fixture-v2"
DIAGNOSTIC = "mixed-distinct"


def read(path):
    return json.loads(path.read_text(encoding="utf-8"))


def expectations(fixture):
    result = {}
    for name, reference in fixture["answers"].items():
        columns = reference["columns"]
        if "integer" in reference:
            bag = Counter({(typed_integer(reference["integer"]),): 1})
        elif "terms" in reference:
            bag = Counter((term_key(term),) for term in reference["terms"])
        else:
            bag = Counter((term_key(term), typed_integer(count)) for term, count in reference["rows"])
        result[name] = columns, bag
    return result


def compare_source(fixture, source, completed):
    if source["java"] != "25.0.5.1+2-LTS" or source["engine_version"] != "v2.0.0":
        raise ValueError("unexpected Java or DuckDB version")
    if not isinstance(source["driver_version"], str) or not source["driver_version"].strip():
        raise ValueError("missing JDBC metadata version")
    if source["schema"] != [
        ["s", "VARCHAR"], ["p", "VARCHAR"], ["kind", "UTINYINT"],
        ["lexical", "VARCHAR"], ["datatype", "VARCHAR"], ["lang", "VARCHAR"],
    ]:
        raise ValueError("production projection drift")
    if (source["positive_rows"] != len(fixture["positive"]["rows"])
            or source["positive_invalid"] != fixture["positive"]["expected_invalid"]
            or source["negative_invalid"] != fixture["negative"]["expected_invalid"]
            or source["negative_ontop_started"] is not False
            or source["restored_positive_rows"] != len(fixture["positive"]["rows"])
            or source["restored_positive_invalid"] != fixture["positive"]["expected_invalid"]):
        raise ValueError("missing or wrong source-row and invalid-row controls")
    if {field: source[field] for field in (
            "view_catalog_entries", "positive_view_delta", "positive_view_invalid",
            "negative_view_delta", "negative_view_invalid", "restored_view_delta",
    )} != {
        "view_catalog_entries": 1, "positive_view_delta": 0, "positive_view_invalid": 0,
        "negative_view_delta": 0, "negative_view_invalid": fixture["negative"]["expected_invalid"],
        "restored_view_delta": 0,
    }:
        raise ValueError("the flattened SQL view or its bag and invalid-row controls differ")
    wanted = fixture["positive"]["domains"]
    seen = source["domains"]
    if len(seen) != len(wanted) or {tuple(sorted(item.items())) for item in seen} != {
            tuple(sorted(item.items())) for item in wanted}:
        raise ValueError("mapping domain coverage changed")
    if completed != {"jdbc_closed": True, "repository_closed": True, "cases": sorted(fixture["queries"]),
                     "scored": False, "full_workload_qualified": False}:
        raise ValueError("missing completion or close markers")


def audit(out):
    fixture = read(FIXTURE / "fixture.json")
    errors = []
    try:
        compare_source(fixture, read(out / "jdbc-source.json"), read(out / "completed.json"))
    except (OSError, KeyError, TypeError, ValueError) as error:
        errors.append(str(error))
    checks = {}
    for name, (columns, expected) in expectations(fixture).items():
        try:
            record = read(out / (name + ".json"))
            if record["query"] != fixture["queries"][name]:
                raise ValueError("query identity differs")
            if "error" in record or "error_class" in record:
                raise ValueError(f"query failed: {record.get('error_class', '')}")
            observed = answer_bag(record["answer"], columns)
            check = {"matches_reference": observed == expected}
            if name == DIAGNOSTIC and len(observed) == 1 and sum(observed.values()) == 1:
                ((kind, lexical, datatype, lang),), = observed
                if kind == "literal" and datatype == typed_integer(0)[2] and lang is None:
                    check["observed_integer_lexical"] = lexical
            checks[name] = check
        except (OSError, KeyError, TypeError, ValueError) as error:
            checks[name] = {"matches_reference": False, "error": str(error)}
    plans = {}
    for name in (DIAGNOSTIC, "resource-distinct"):
        try:
            path = out / (name + "-plan.json")
            plan = read(path)
            if plan["query"] != fixture["queries"][name]:
                raise ValueError("reformulation query identity differs")
            plans[name] = {
                "sha256": sha256(path.read_bytes()).hexdigest(),
                "iq_available": bool(plan.get("iq")),
                "native_sql_available": bool(plan.get("native_query")),
            }
        except (OSError, KeyError, TypeError, ValueError) as error:
            plans[name] = {"error": str(error)}
    return {
        "required_pass": not errors and all(check["matches_reference"] for name, check in checks.items()
                                            if name != DIAGNOSTIC),
        "source_errors": errors,
        "typed_answers": checks,
        "reformulations": plans,
        "scored": False,
        "full_workload_qualified": False,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    result = audit(parser.parse_args().output)
    print(json.dumps(result, indent=2, sort_keys=True))
    raise SystemExit(0 if result["required_pass"] else 1)
