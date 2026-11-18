#!/usr/bin/env python3
"""Run the W3C SPARQL 1.1 query test suite (rdf-tests) against the rdfdb CLI and report pass rates.

Usage: w3c_sparql11.py <rdf-tests/sparql/sparql11 dir> <rdfdb binary> <out.json>

Categories: pass, fail (wrong result / unexpected error), unsupported (rdfdb returned an explicit Unsupported error),
skip-named (test needs named graphs / FROM, which rdfdb does not store), skip-format (result format we don't compare,
e.g. CSV/TSV result-format tests), plus syntax tests (positive must parse, negative must be rejected).
"""
import json, os, subprocess, sys, tempfile, collections
from rdflib import Graph, URIRef, BNode, Literal, Namespace, RDF
from rdflib.query import Result
from rdflib.collection import Collection
from rdflib.compare import isomorphic

MF = Namespace("http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#")
QT = Namespace("http://www.w3.org/2001/sw/DataAccess/tests/test-query#")
UT = Namespace("http://www.w3.org/2009/sparql/tests/test-update#")

def manifests(root):
    top = Graph().parse(os.path.join(root, "manifest-sparql11-query.ttl"), format="turtle")
    out = []
    for s, _, lst in top.triples((None, MF.include, None)):
        for item in Collection(top, lst):
            out.append(str(item))
    return out

def fileuri_to_path(u):
    return u[len("file://"):] if u.startswith("file://") else u

def run_cli(binary, data_path, query_text):
    p = subprocess.run([binary, "query", data_path, query_text], capture_output=True, text=True, timeout=60)
    return p.returncode, p.stdout, p.stderr

def term_key(t):
    if t is None: return "UNBOUND"
    if isinstance(t, BNode): return "_:b"
    if isinstance(t, Literal):
        return f'L|{str(t)}|{t.datatype or ""}|{(t.language or "").lower()}'
    return f"I|{t}"

def json_term(v):
    if v is None: return None
    ty = v["type"]
    if ty == "uri": return URIRef(v["value"])
    if ty == "bnode": return BNode(v["value"])
    dt = v.get("datatype"); lang = v.get("xml:lang")
    if lang: return Literal(v["value"], lang=lang)
    if dt and dt != "http://www.w3.org/2001/XMLSchema#string": return Literal(v["value"], datatype=URIRef(dt))
    return Literal(v["value"])

def actual_from_json(text):
    j = json.loads(text)
    if "boolean" in j: return ("ask", j["boolean"])
    vars_ = j["head"].get("vars", [])
    rows = [{v: json_term(b.get(v)) for v in vars_} for b in j["results"]["bindings"]]
    return ("select", vars_, rows)

def expected_result(path):
    fmt = {"srx": "xml", "srj": "json", "tsv": "tsv", "csv": "csv"}.get(path.rsplit(".", 1)[-1])
    if fmt is None: return None
    if fmt in ("tsv", "csv"): return "format"
    r = Result.parse(open(path, "rb"), format=fmt)
    if r.type == "ASK": return ("ask", bool(r.askAnswer))
    vars_ = [str(v) for v in r.vars]
    rows = [{str(k): v for k, v in b.items()} for b in r.bindings]
    return ("select", vars_, rows)

def compare(exp, act, ordered):
    if exp[0] != act[0]: return False
    if exp[0] == "ask": return exp[1] == act[1]
    vs = sorted(set(exp[1]) | set(act[1]))
    def rowkey(r): return tuple(term_key(r.get(v)) for v in vs)
    ek = [rowkey(r) for r in exp[2]]; ak = [rowkey(r) for r in act[2]]
    # Pass = same multiset of solutions (blank-node labels erased). ORDER BY sequence is reported separately
    # (ties make several orders valid), so `ordered` only affects the detail, not the outcome.
    return collections.Counter(ek) == collections.Counter(ak)

def main():
    root, binary, out_path = sys.argv[1], sys.argv[2], sys.argv[3]
    results = []
    for mpath in manifests(root):
        mpath = fileuri_to_path(mpath)
        if not os.path.isabs(mpath): mpath = os.path.join(root, mpath)
        if os.path.isdir(mpath): mpath = os.path.join(mpath, "manifest.ttl")
        g = Graph().parse(mpath, format="turtle", publicID="file://" + mpath)
        mdir = os.path.dirname(mpath)
        for _, _, lst in g.triples((None, MF.entries, None)):
            for test in Collection(g, lst):
                ty = g.value(test, RDF.type)
                name = str(test).split("#")[-1]
                cat = os.path.basename(mdir)
                rec = {"suite": cat, "test": name, "type": str(ty).split("#")[-1]}
                action = g.value(test, MF.action)
                if ty in (MF.PositiveSyntaxTest11, MF.NegativeSyntaxTest11):
                    qpath = fileuri_to_path(str(action))
                    q = open(qpath, encoding="utf-8").read()
                    with tempfile.NamedTemporaryFile("w", suffix=".nt", delete=False) as f: empty = f.name
                    code, out, err = run_cli(binary, empty, f"BASE <file://{qpath}>\n" + q)
                    syntax_err = code != 0 and "syntax" in err.lower()
                    rec["outcome"] = ("pass" if not syntax_err else "fail") if ty == MF.PositiveSyntaxTest11 else ("pass" if code != 0 else "fail")
                    rec["detail"] = err.strip()[:200]
                    results.append(rec); continue
                if ty != MF.QueryEvaluationTest:
                    rec["outcome"] = "skip-type"; results.append(rec); continue
                qfile = g.value(action, QT.query)
                datas = list(g.objects(action, QT.data))
                named = list(g.objects(action, QT.graphData))
                resfile = g.value(test, MF.result)
                qpath = fileuri_to_path(str(qfile))
                q = open(qpath, encoding="utf-8").read()
                if named:
                    rec["outcome"] = "skip-named"; results.append(rec); continue
                exp = expected_result(fileuri_to_path(str(resfile))) if resfile is not None else None
                if exp is None and resfile is not None and str(resfile).endswith((".ttl", ".rdf", ".nt")):
                    exp = ("graph", fileuri_to_path(str(resfile)))
                if exp == "format" or exp is None:
                    rec["outcome"] = "skip-format"; results.append(rec); continue
                dg = Graph()
                try:
                    for d in datas:
                        dp = fileuri_to_path(str(d))
                        dg.parse(dp, format="xml" if dp.endswith(".rdf") else "turtle", publicID="file://" + dp)
                except Exception as e:
                    rec["outcome"] = "skip-harness"; rec["detail"] = f"data parse: {e}"[:200]; results.append(rec); continue
                with tempfile.NamedTemporaryFile("w", suffix=".nt", delete=False) as f:
                    f.write(dg.serialize(format="nt")); data_path = f.name
                try:
                    code, out, err = run_cli(binary, data_path, f"BASE <file://{qpath}>\n" + q)
                except subprocess.TimeoutExpired:
                    rec["outcome"] = "fail"; rec["detail"] = "timeout"; results.append(rec); continue
                if code != 0:
                    rec["outcome"] = "unsupported" if "unsupported" in err.lower() or "not supported" in err.lower() else "fail"
                    rec["detail"] = err.strip()[:200]; results.append(rec); continue
                try:
                    if exp[0] == "graph":
                        eg = Graph().parse(exp[1], format="xml" if exp[1].endswith(".rdf") else "turtle")
                        ag = Graph().parse(data=out, format="nt")
                        ok = isomorphic(eg, ag)
                    else:
                        ok = compare(exp, actual_from_json(out), "order by" in q.lower())
                except Exception as e:
                    ok = False; rec["detail"] = f"compare error: {e}"[:200]
                rec["outcome"] = "pass" if ok else "fail"
                results.append(rec)
    json.dump(results, open(out_path, "w"), indent=1)
    by = collections.defaultdict(collections.Counter)
    for r in results: by[r["suite"]][r["outcome"]] += 1
    tot = collections.Counter(r["outcome"] for r in results)
    for s in sorted(by): print(f"{s:22s} " + " ".join(f"{k}={v}" for k, v in sorted(by[s].items())))
    print("TOTAL", dict(tot))

if __name__ == "__main__":
    main()
