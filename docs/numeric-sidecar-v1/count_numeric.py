#!/usr/bin/env python3
"""#85 part A: per numeric XSD datatype, how many literal occurrences / distinct terms are in canonical form and (integers) fit 60 bits.
usage: count_numeric.py file.nt out.json"""
import re, sys, json, time, collections
INT_TYPES = {"integer","int","long","short","byte","nonNegativeInteger","positiveInteger","negativeInteger","nonPositiveInteger","unsignedInt","unsignedLong","unsignedShort","unsignedByte"}
NUM_TYPES = INT_TYPES | {"decimal","double","float"}
INT_RE = re.compile(r'^(0|-?[1-9][0-9]*)$')
DEC_PLAIN = re.compile(r'^-?(0|[1-9][0-9]*)(\.[0-9]*[1-9])?$')        # no trailing fraction zeros, no ".0"
DEC_XSD11 = re.compile(r'^-?(0|[1-9][0-9]*)\.(0|[0-9]*[1-9])$')        # XSD 1.1 canonical: always has a point
FLT_RE = re.compile(r'^(-?0\.0E0|-?([1-9]\.[0-9]*[1-9]|[1-9]\.0)E(-?[1-9][0-9]*|0)|NaN|INF|-INF)$')
LIT = re.compile(rb'^"((?:[^"\\]|\\.)*)"\^\^<http://www\.w3\.org/2001/XMLSchema#([A-Za-z]+)>')
def canon(dt, lex):
    if dt in INT_TYPES: return bool(INT_RE.match(lex))
    if dt == "decimal": return bool(DEC_PLAIN.match(lex))
    return bool(FLT_RE.match(lex))
def main(path, out):
    t0 = time.time()
    total_lines = literals = 0
    occ = collections.Counter(); can = collections.Counter(); can11 = collections.Counter(); fit60 = collections.Counter(); bad_distinct = collections.defaultdict(set)
    distinct = collections.defaultdict(set); distinct_can = collections.defaultdict(set)
    with open(path, "rb") as f:
        for line in f:
            total_lines += 1
            parts = line.split(b" ", 2)
            if len(parts) < 3: continue
            obj = parts[2]
            if obj[:1] == b'"':
                literals += 1
                if b"^^<http://www.w3.org/2001/XMLSchema#" in obj:
                    m = LIT.match(obj)
                    if m and m.group(2).decode() in NUM_TYPES:
                        lex = m.group(1).decode(); dt = m.group(2).decode()
                        occ[dt] += 1
                        distinct[dt].add(lex)
                        c = canon(dt, lex)
                        if c:
                            can[dt] += 1; distinct_can[dt].add(lex)
                            if dt in INT_TYPES and abs(int(lex)) < (1 << 59): fit60[dt] += 1
                        if dt == "decimal" and DEC_XSD11.match(lex): can11[dt] += 1
                        if dt in INT_TYPES: c11 = c
                        elif dt == "decimal": c11 = bool(DEC_XSD11.match(lex))
                        else: c11 = c
                        if not c and len(bad_distinct[dt]) < 5: bad_distinct[dt].add(lex)
                        if dt in ("decimal",) and not c11 and len(bad_distinct[dt+"_xsd"]) < 5: bad_distinct[dt+"_xsd"].add(lex)
    res = {"file": path.split("/")[-1], "lines": total_lines, "literals": literals, "seconds": round(time.time()-t0,1), "types": {}}
    for dt in sorted(occ):
        res["types"][dt] = {"occurrences": occ[dt], "distinct": len(distinct[dt]), "canonical_occ": can[dt], "canonical_distinct": len(distinct_can[dt]),
                            "fits_60bit_occ": fit60[dt] if dt in INT_TYPES else None, "xsd11_canonical_occ": can11[dt] if dt == "decimal" else None,
                            "not_roundtrip_plain_rule_occ": occ[dt]-can[dt],
                            "not_roundtrip_xsd_rule_occ": (occ[dt]-can11[dt]) if dt == "decimal" else occ[dt]-can[dt],
                            "examples_failing": sorted(bad_distinct[dt]) + sorted(bad_distinct.get(dt+"_xsd", []))}
    n = sum(occ.values()); c = sum(can.values())
    res["numeric_occ"] = n; res["numeric_canonical_occ"] = c
    res["numeric_share_of_literals"] = round(n/literals, 6) if literals else None
    res["canonical_share_of_numeric"] = round(c/n, 6) if n else None
    json.dump(res, open(out, "w"), indent=1)
    print(json.dumps(res, indent=1))
main(sys.argv[1], sys.argv[2])
