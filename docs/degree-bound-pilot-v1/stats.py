# Offline exact degree statistics for #99, one pass over an N-Triples file (read in place).
import sys, json, math, collections
f = sys.argv[1]; out = sys.argv[2]
U = "http://www."
consts = [  # (kind, predicate-local, constant IRI) used by LUBM queries 1-14
 ("o","takesCourse","<http://www.Department0.University0.edu/GraduateCourse0>"),
 ("o","publicationAuthor","<http://www.Department0.University0.edu/AssistantProfessor0>"),
 ("o","worksFor","<http://www.Department0.University0.edu>"),
 ("o","memberOf","<http://www.Department0.University0.edu>"),
 ("s","teacherOf","<http://www.Department0.University0.edu/AssociateProfessor0>"),
 ("o","subOrganizationOf","<http://www.University0.edu>"),
 ("o","subOrganizationOf","<http://www.Department0.University0.edu>"),
 ("o","undergraduateDegreeFrom","<http://www.University0.edu>"),
]
sub = collections.defaultdict(collections.Counter)
obj = collections.defaultdict(collections.Counter)
n = 0
with open(f, "rb") as fh:
    for line in fh:
        line = line.rstrip(b"\r\n")
        if not line or line[0] == 35: continue
        a = line.find(b" "); b = line.find(b" ", a + 1)
        s = line[:a]; p = line[a+1:b]; o = line[b+1:]
        if o.endswith(b" ."): o = o[:-2]
        sub[p][s] += 1; obj[p][o] += 1; n += 1
        # duplicates would inflate degrees; checked separately (see count of distinct below)
res = {"file": f, "lines": n, "preds": {}}
for p in sub:
    ps = p.decode()
    ds = sub[p]; do = obj[p]
    N = sum(ds.values())
    res["preds"][ps] = {
      "N": N, "n_subj": len(ds), "n_obj": len(do),
      "Ds": max(ds.values()), "Do": max(do.values()),
      "l2s": math.sqrt(sum(v*v for v in ds.values())), "l2o": math.sqrt(sum(v*v for v in do.values())),
    }
T = b"<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>"
res["class_counts"] = {k.decode(): v for k, v in obj[T].items()}
res["const"] = {}
for kind, pl, c in consts:
    for p in sub:
        if p.decode().endswith("#" + pl + ">"):
            tab = obj[p] if kind == "o" else sub[p]
            res["const"][f"{kind}|{pl}|{c}"] = tab.get(c.encode(), 0)
json.dump(res, open(out, "w"), indent=1)
print(n, len(res["preds"]))
