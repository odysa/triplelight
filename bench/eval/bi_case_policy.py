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
    previous = sys.dont_write_bytecode
    sys.dont_write_bytecode = True
    try:
        validator = importlib.import_module("http_measure_value")
        qualify = importlib.import_module("qualify")
        diagnostic = importlib.import_module("value_diagnostic")
        return validator, qualify, diagnostic
    finally:
        sys.dont_write_bytecode = previous
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
        self.reference_digests = None
        self.diagnostic_files = {}
        reference_dir = registry / "references" / workload
        for case, query in self.queries.items():
            files = policy["reference_files"]
            name = case + ".json"
            self.references[case] = checked_json(reference_dir / name, files[name])
            if query["template"] == 7:
                name = case + "-diagnostic.json"
                self.diagnostic_files[case] = (reference_dir / name, files[name])

    def bind_references(self, records, fold):
        """Verify the driver's compact reference records with its own Fold class."""
        if records.keys() != self.references.keys():
            raise ValueError("Reference case inventory differs from the frozen BI workload")
        digests = {}
        for case, reference in self.references.items():
            accumulator = fold(reference["head"]["vars"])
            for row in reference["results"]["bindings"]:
                accumulator.add(row)
            expected = accumulator.result()
            if any(records[case].get(key) != value for key, value in expected.items()):
                raise ValueError(f"Reference digest differs from the frozen BI answer for {case}")
            digests[case] = expected
        self.reference_digests = digests

    def check(self, case_id, observed_answer, reference_record):
        """Return a class and reason after checking the full typed SPARQL result."""
        if case_id not in self.queries:
            return "mismatch", "Case is outside the frozen BI workload"
        expected = self.references[case_id]
        if self.reference_digests is None:
            matches = reference_record == expected
        else:
            matches = isinstance(reference_record, dict) and all(
                reference_record.get(key) == value
                for key, value in self.reference_digests[case_id].items()
            )
        if not matches:
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


NAME = "bsbm-bi-frozen-v1"
REGISTRATIONS = {
    "734c77aedb9cdd72fa09b1b96d816230343f18a5366b3501067b073211275892": (
        "bi10m", "3a207c6725820036eec87d08dd414090d3548ac2839ca192414dad6e352f680e",
    ),
    "7b3f640bbab516db80ceba87549b9670802a93c5ff77181bdda66c0855fc3dfb": (
        "bi100m", "6e4ef939f05b15677802d17257096de9da32a452fe33dd40c07d0b0537ee430d",
    ),
}
REGISTRY = Path(__file__).resolve().parent / "registry"
FREEZE_PATH = REGISTRY / "policies/value-policy-freeze.json"
FREEZE_SHA256 = hashlib.sha256(FREEZE_PATH.read_bytes()).hexdigest() if FREEZE_PATH.is_file() else None
REGISTRATION = REGISTRATIONS.get(FREEZE_SHA256)
DATASET_SHA256 = REGISTRATION[1] if REGISTRATION else []
_adapter = None


def start(context):
    """Bind the driver to one published BI workload before any requests are sent."""
    global _adapter
    _adapter = None
    if Path(context["bundle_dir"]).resolve() != REGISTRY.parent or not REGISTRATION:
        raise ValueError("The BI bundle must contain the adapter at its root and a registered policy")
    workload, dataset_sha256 = REGISTRATION
    if context["dataset_sha256"] != [dataset_sha256]:
        raise ValueError("The run dataset differs from the frozen BI policy")
    adapter = BICasePolicy(REGISTRY, workload, FREEZE_SHA256, dataset_sha256, context["case_queries"])
    adapter.bind_references(context["reference_records"], context["fold"])
    _adapter = adapter


def check(case_id, observed_answer, reference_record):
    """Driver callback for answers that failed the exact comparison."""
    if _adapter is None:
        raise RuntimeError("BI case policy has not been started")
    result = {
        "head": {"vars": observed_answer["vars"]},
        "results": {"bindings": observed_answer["rows"]},
    }
    status, reason = _adapter.check(case_id, result, reference_record)
    if status == "pass":
        number = _adapter.queries[case_id]["template"]
        if number in _adapter.qualify.ORDER_KEYS:
            reason = "Same typed answer and a sequence satisfying the frozen BI ORDER BY rules"
        else:
            reason = "Same typed answer and no outer ORDER BY in the frozen BI case"
        status = "value-compatible"
    return status, reason
