#!/usr/bin/env python3
"""Run the W3C SPARQL 1.1 entailment tests that list the RDFS regime against `triplelight query --entailment=rdfs`.

Usage: w3c_rdfs_entailment.py <rdf-tests/sparql/sparql11 dir> <triplelight binary> <out.json> [expected-differences]

TripleLight's RDFS view (`Entailment::Rdfs`, #108) applies rdfs2, 3, 5, 7, 9 and 11, without axiomatic triples, which is
less than the SPARQL 1.1 RDFS entailment regime the suite assumes. A case the rule set cannot pass is listed in the
expected-differences file (one `case-id  reason` per line). The run fails on an unlisted failure, and on a listed case
that now passes, so the list can only shrink.
"""
import json, os, subprocess, sys, collections, tempfile
from rdflib import Graph, Namespace, RDF
from rdflib.collection import Collection
from rdflib.compare import isomorphic
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from w3c_sparql11 import MF, QT, fileuri_to_path, expected_result, actual_from_json, compare

SD = Namespace("http://www.w3.org/ns/sparql-service-description#")
RDFS_REGIME = "http://www.w3.org/ns/entailment/RDFS"

def regimes(g, action):
    value = g.value(action, SD.entailmentRegime)
    if value is None: return set()
    try: return {str(v) for v in Collection(g, value)} or {str(value)}
    except Exception: return {str(value)}

def run(binary, data_path, query):
    p = subprocess.run([binary, "query", data_path, query, "--entailment=rdfs"], capture_output=True, text=True, timeout=60)
    return p.returncode, p.stdout, p.stderr

def main():
    root, binary, out_path = sys.argv[1:4]
    listed = {}
    if len(sys.argv) > 4:
        for line in open(sys.argv[4], encoding="utf-8"):
            line = line.split("#", 1)[0].strip()
            if line: case, _, reason = line.partition(" "); listed[case] = reason.strip()
    mpath = os.path.join(root, "entailment", "manifest.ttl")
    g = Graph().parse(mpath, format="turtle", publicID="file://" + mpath)
    results, scratch = [], tempfile.mkdtemp(prefix="w3c-rdfs-")
    for _, _, lst in g.triples((None, MF.entries, None)):
        for test in Collection(g, lst):
            name = str(test).split("#")[-1]
            action = g.value(test, MF.action)
            if g.value(test, RDF.type) != MF.QueryEvaluationTest or RDFS_REGIME not in regimes(g, action): continue
            rec = {"test": name}
            qpath = fileuri_to_path(str(g.value(action, QT.query)))
            q = open(qpath, encoding="utf-8").read()
            resfile = g.value(test, MF.result)
            exp = expected_result(fileuri_to_path(str(resfile))) if resfile is not None else None
            dg = Graph()
            for d in g.objects(action, QT.data):
                dp = fileuri_to_path(str(d))
                dg.parse(dp, format="xml" if dp.endswith(".rdf") else "turtle", publicID="file://" + dp)
            data_path = os.path.join(scratch, f"{name}.nt")
            open(data_path, "w").write(dg.serialize(format="nt"))
            code, out, err = run(binary, data_path, f"BASE <file://{qpath}>\n" + q)
            if code != 0:
                rec["outcome"] = "unsupported" if "unsupported" in err.lower() else "fail"; rec["detail"] = err.strip()[:300]
            elif exp in (None, "format"):
                rec["outcome"] = "skip-format"
            else:
                try:
                    if exp[0] == "graph":
                        ok = isomorphic(exp[1], Graph().parse(data=out, format="nt"))
                    else:
                        ok = compare(exp, actual_from_json(out), False)
                except Exception as e:
                    ok = False; rec["detail"] = f"compare error: {e}"[:300]
                rec["outcome"] = "pass" if ok else "fail"
                if not ok: rec["actual"] = out[:1500]
            rec["listed"] = listed.get(name)
            results.append(rec)
    json.dump(results, open(out_path, "w"), indent=1)
    bad = []
    for r in results:
        missed = r["outcome"] in ("fail", "unsupported")
        if missed and r["listed"] is None: bad.append(f"unexpected {r['outcome']}: {r['test']} {r.get('detail', '')[:120]}")
        if not missed and r["listed"] is not None: bad.append(f"listed but passes now: {r['test']}")
    seen = {r["test"] for r in results}
    bad += [f"listed but not in the suite: {c}" for c in listed if c not in seen]
    tally = collections.Counter(("expected-difference" if r["listed"] and r["outcome"] in ("fail", "unsupported") else r["outcome"]) for r in results)
    print("RDFS entailment cases:", len(results), dict(sorted(tally.items())))
    for line in bad: print(line)
    sys.exit(1 if bad else 0)

if __name__ == "__main__":
    main()
