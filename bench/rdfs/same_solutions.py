"""Compare two SPARQL JSON results as multisets of solutions; blank nodes are one placeholder. Prints one line."""
import collections, json, sys

def solutions(path):
    doc = json.load(open(path))
    names = doc["head"]["vars"]
    def term(b):
        if b is None: return None
        if b["type"] == "bnode": return ("bnode",)
        if b["type"] == "literal":
            return ("literal", b["value"], b.get("datatype", ""), b.get("xml:lang", ""))
        return (b["type"], b["value"])
    rows = collections.Counter(tuple(term(r.get(v)) for v in sorted(names)) for r in doc["results"]["bindings"])
    return sorted(names), rows

name, a, b = sys.argv[1:]
(va, ra), (vb, rb) = solutions(a), solutions(b)
same = va == vb and ra == rb
print(f"{name}: jena {sum(ra.values())} tl {sum(rb.values())} {'same' if same else 'DIFFER'}")
if not same:
    for row, n in list((ra - rb).items())[:5]: print("  only jena", n, row)
    for row, n in list((rb - ra).items())[:5]: print("  only tl  ", n, row)
sys.exit(0 if same else 1)
