"""Common, lossless SELECT result boundary for embedded Python benchmarks."""
from decimal import Decimal
import hashlib
import json

XSD = "http://www.w3.org/2001/XMLSchema#"


def term(kind, value, datatype=None, lang=None):
    if kind not in ("uri", "bnode", "literal") or not isinstance(value, str):
        raise ValueError("Invalid RDF term")
    if kind != "literal" and (datatype or lang):
        raise ValueError("Resource has literal metadata")
    if lang:
        if datatype not in (None, "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString"):
            raise ValueError("Language literal has incompatible datatype")
        return (kind, value, None, lang.lower())
    return (kind, value, None if datatype == XSD + "string" else datatype, None)


def json_term(value):
    if value is None:
        return None
    kind = value["type"]
    return term("literal" if kind == "typed-literal" else kind, value["value"],
                value.get("datatype"), value.get("xml:lang"))


def python_term(value, api):
    """Invert only the public binding's explicitly lossless convenience conversions."""
    if value is None:
        return None
    if isinstance(value, api.IRI):
        return term("uri", str(value))
    if isinstance(value, api.BNode):
        return term("bnode", str(value))
    if isinstance(value, api.Literal):
        return term("literal", value.value, value.datatype, value.lang)
    if type(value) is bool:
        return term("literal", "true" if value else "false", XSD + "boolean")
    if type(value) is int:
        return term("literal", str(value), XSD + "integer")
    if type(value) is str:
        return term("literal", value)
    raise TypeError(f"Binding returned a term without a lossless adapter: {type(value)}")


def signature(variables, rows):
    """Called after complete consumption, outside all reported query timings.

    Variable alignment, bag multiplicity and term identity all enter the digest.
    Blank labels are not graph-isomorphism normalized: callers must use datasets
    without blank nodes or supply a separately validated common-label mapping.
    """
    if len(set(variables)) != len(variables):
        raise ValueError("Duplicate projected variable")
    order = sorted(range(len(variables)), key=lambda i: variables[i])
    encoded = []
    for row in rows:
        if len(row) != len(variables):
            raise ValueError("Result width mismatch")
        encoded.append(json.dumps([row[i] for i in order], ensure_ascii=False,
                                  separators=(",", ":")))
    header = json.dumps(sorted(variables), separators=(",", ":")) + "\n"
    digest = hashlib.sha256(header.encode())
    for row in sorted(encoded):
        digest.update(row.encode())
        digest.update(b"\n")
    return {"vars": sorted(variables), "rows": len(rows), "sha256": digest.hexdigest()}


def check_order(variables, rows, spec):
    """Check projected ORDER BY keys, allowing arbitrary order within tied keys.

    Explicit specs avoid pretending to implement all SPARQL ordering semantics.
    Only homogeneous resource/string lexical keys and finite numeric literals
    are supported. Unsupported key types/hidden expressions fail closed.
    """
    indexes = [(variables.index(s["var"]), s) for s in spec]

    def key(t, s):
        if t is None:
            return (0, None)
        if s["kind"] == "numeric":
            numeric = {"integer", "int", "long", "short", "byte", "decimal", "double", "float",
                       "nonNegativeInteger", "positiveInteger", "nonPositiveInteger", "negativeInteger",
                       "unsignedLong", "unsignedInt", "unsignedShort", "unsignedByte"}
            if t[0] != "literal" or t[2] not in {XSD + n for n in numeric}:
                raise ValueError("Non-numeric ORDER BY key")
            value = Decimal(t[1])
            if not value.is_finite():
                raise ValueError("Non-finite ORDER BY requires a separate comparator")
        elif s["kind"] == "lexical":
            if t[0] != s["term_kind"] or t[2] != s.get("datatype") or t[3] is not None:
                raise ValueError("ORDER BY lexical key type differs from declared homogeneous type")
            value = t[1]
        else:
            raise ValueError("Unsupported ORDER BY comparator")
        return (1, value)

    previous = None
    for row in rows:
        current = [key(row[i], s) for i, s in indexes]
        if previous is not None:
            for a, b, (_, s) in zip(previous, current, indexes):
                if a == b:
                    continue
                if (a > b) != bool(s.get("descending", False)):
                    raise ValueError("ORDER BY sequence is not monotonic")
                break
        previous = current
    return {"checked": True, "keys": spec, "ties": "arbitrary row order permitted within equal keys"}
