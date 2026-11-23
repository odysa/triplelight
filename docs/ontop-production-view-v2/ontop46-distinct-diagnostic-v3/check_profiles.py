#!/usr/bin/env python3
"""Replay the diagnostic from typed RDF4J records and archived Jena terms."""
import argparse
from collections import Counter
import json
from pathlib import Path
import re

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


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("attempt", type=Path)
    args = parser.parse_args()
    result = evaluate(Path(__file__).resolve().parent / "source", args.attempt)
    print(json.dumps(result, indent=2))
