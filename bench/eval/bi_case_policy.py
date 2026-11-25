"""Adapter for the published, frozen BSBM BI answer validators."""

from collections import Counter
import hashlib
import importlib
import json
from pathlib import Path
import sys


def checked_json(path, expected_sha256):
    raw = path.read_bytes()
    if hashlib.sha256(raw).hexdigest() != expected_sha256:
        raise ValueError(f"Frozen BI digest differs for {path.name}")
    return json.loads(raw)


def load_validator(source, policy, numeric):
    hashes = dict(policy["validator_sources"])
    for name, digest in numeric["validator_sources"].items():
        if hashes.setdefault(name, digest) != digest:
            raise ValueError(f"Conflicting frozen BI source hashes for {name}")
    required = {
        "bi_sql.py", "envelope_qualify.py", "fixture_check.py", "float_envelope.py",
        "http_measure.py", "http_measure_value.py", "qualify.py",
        "value_diagnostic.py", "workload_check.py",
    }
    if not required <= hashes.keys():
        raise ValueError("The BI validator source freeze is incomplete")
    for name, digest in hashes.items():
        path = source / name
        if Path(name).name != name or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError(f"Frozen BI validator source differs for {name}")
        loaded = sys.modules.get(path.stem)
        if loaded is not None and Path(getattr(loaded, "__file__", "") or "").resolve() != path.resolve():
            raise ValueError(f"Module {path.stem} is already loaded from another path")
    sys.path.insert(0, str(source))
    try:
        validator = importlib.import_module("http_measure_value")
        qualify = importlib.import_module("qualify")
        diagnostic = importlib.import_module("value_diagnostic")
        return validator, qualify, diagnostic
    finally:
        sys.path.pop(0)


class BICasePolicy:
    def __init__(self, registry, workload, policy_sha256, dataset_sha256, query_hashes):
        """Bind the adapter to the verified runtime dataset and query inventory.

        `registry` is the extracted registry directory. `workload` is its workload
        name, such as bi10m. `query_hashes` maps the driver's case IDs to SHA-256
        digests of the actual query files.
        """
        registry = Path(registry).resolve()
        if Path(workload).name != workload or not workload:
            raise ValueError("Invalid frozen BI workload name")
        policy_path = registry / "policies/value-policy-freeze.json"
        policy = checked_json(policy_path, policy_sha256)
        manifest_sha256 = policy["workload_manifest_sha256"]
        manifest = checked_json(registry / "workloads" / workload / "manifest.json", manifest_sha256)
        if manifest["source_data_sha256"] != dataset_sha256:
            raise ValueError("Frozen BI policy belongs to a different dataset")
        self.queries = {Path(q["file"]).stem: q for q in manifest["queries"]}
        expected = {case: query["sha256"] for case, query in self.queries.items()}
        if len(self.queries) != len(manifest["queries"]) or query_hashes != expected:
            raise ValueError("Frozen BI policy belongs to a different query workload")

        numeric_dir = registry / "policies/numeric"
        numeric_sha256 = policy["numeric_freeze_sha256"]
        numeric = checked_json(numeric_dir / "freeze.json", numeric_sha256)
        self.validator, self.qualify, self.diagnostic = load_validator(
            registry / "validator-source", policy, numeric,
        )
        self.validator.check_policy(policy_path, policy_sha256, manifest_sha256)
        frozen = self.validator.FrozenInputs(numeric_dir, numeric_sha256, dataset_sha256, manifest_sha256)
        self.inputs = {
            case: frozen.for_instance(query)
            for case, query in self.queries.items() if query["template"] in (4, 5, 6)
        }
        self.references = {}
        self.diagnostic_files = {}
        reference_dir = registry / "references" / workload
        for case, query in self.queries.items():
            files = policy["reference_files"]
            name = case + ".json"
            self.references[case] = checked_json(reference_dir / name, files[name])
            if query["template"] == 7:
                name = case + "-diagnostic.json"
                self.diagnostic_files[case] = (reference_dir / name, files[name])

    def check(self, case_id, observed_answer, reference_record):
        """Return a class and reason after checking the full typed SPARQL result."""
        if case_id not in self.queries:
            return "mismatch", "Case is outside the frozen BI workload"
        expected = self.references[case_id]
        if reference_record != expected:
            return "mismatch", "Reference differs from the frozen BI answer"

        number = self.queries[case_id]["template"]
        variables = expected["head"]["vars"]
        try:
            actual = observed_answer["head"]["vars"]
            rows = observed_answer["results"]["bindings"]
            projected = set(variables)
            if (len(actual) != len(variables) or set(actual) != projected
                    or any(not isinstance(row, dict) or not row.keys() <= projected for row in rows)):
                raise ValueError("Projected variables differ")

            if number == 7:
                self.validator.http_measure.verify(
                    number, observed_answer, expected,
                    checked_json(*self.diagnostic_files[case_id]), None, None,
                )
            else:
                result = self.validator.diagnose(
                    number, observed_answer, expected, self.inputs.get(case_id),
                )
                if result["order"]["status"] != "pass":
                    raise ValueError(result["order"]["error"])

            reference_rows = expected["results"]["bindings"]
            if self.bag(rows, variables) == self.bag(reference_rows, variables):
                return "pass", "Typed answer matches the frozen BI reference"
            floats = self.qualify.FLOAT_COLUMNS.get(number, set())
            if floats and self.bag(rows, variables, floats) == self.bag(reference_rows, variables, floats):
                return "value-compatible", "Different xsd:float spelling at the same binary32 value"

            counts = self.qualify.COUNT_COLUMNS.get(number, set())
            if counts and self.bag(rows, variables, counts=counts) == self.bag(reference_rows, variables, counts=counts):
                return "value-compatible", "Declared COUNT column has the same exact nonnegative integer value"
            if number == 7:
                reason = "Selection satisfies the frozen Q7 offer-count cutoff and vendor filter"
            elif number in (4, 5, 6):
                reason = "Computed value satisfies the frozen per-entity rounding interval"
            elif number in (3, 8):
                reason = "Computed value is within four binary32 steps of the frozen reference"
            else:
                raise ValueError("No relaxed comparison registered for this BI case")
            return "value-compatible", reason
        except (KeyError, OverflowError, TypeError, ValueError) as error:
            return "mismatch", str(error)

    def bag(self, rows, variables, floats=frozenset(), counts=frozenset()):
        def key(term, variable):
            if term is not None and variable in counts:
                return "count", self.diagnostic.numeric_value(term)
            if (variable in floats and term is not None
                    and term.get("datatype") == self.qualify.XSD + "float"):
                return "binary32", self.diagnostic.numeric_value(term)
            return self.qualify.exact_term(term)

        return Counter(tuple(key(row.get(variable), variable) for variable in variables) for row in rows)
