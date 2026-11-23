# Independent offline oracle for #99: exact result counts (set semantics, distinct triples) by in-memory joins.
import os
QDIR = os.environ.get("LUBM_QUERY_DIR", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "bench", "lubm"))  # q1.rq..q14.rq
import sys, re, json, collections
sys.path.insert(0, ".")
f = sys.argv[1]
UB = "http://swat.cse.lehigh.edu/onto/univ-bench.owl#"; RDFT = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
S = collections.defaultdict(lambda: collections.defaultdict(set)); O = collections.defaultdict(lambda: collections.defaultdict(set))
with open(f, "rb") as fh:
    for line in fh:
        if not line or line[0] != 60: continue
        line = line.rstrip(b"\r\n"); a = line.find(b" "); b = line.find(b" ", a + 1)
        s = line[:a]; p = line[a+1:b][1:-1].decode(); o = line[b+1:]
        if o.endswith(b" ."): o = o[:-2]
        S[p][s].add(o); O[p][o].add(s)
def full(name): return RDFT if name == "rdf:type" else UB + name.split(":")[1]
def term(x):
    if x.startswith("ub:"): return ("<" + UB + x[3:] + ">").encode()
    return x.encode()
def solve(atoms):
    # evaluate by backtracking in textual order with index lookups; count solutions
    def rec(i, env):
        if i == len(atoms): return 1
        s, p, o = atoms[i]; p = full(p)
        sv = env.get(s) if s.startswith("?") else term(s); ov = env.get(o) if o.startswith("?") else term(o)
        n = 0
        if sv is not None and ov is not None:
            return rec(i + 1, env) if ov in S[p].get(sv, ()) else 0
        if sv is not None:
            for x in S[p].get(sv, ()): n += rec(i + 1, {**env, o: x})
        elif ov is not None:
            for x in O[p].get(ov, ()): n += rec(i + 1, {**env, s: x})
        else:
            for a, bs in S[p].items():
                for b in bs: n += rec(i + 1, {**env, s: a, o: b})
        return n
    return rec(0, {})
out = {}
for i in range(1, 15):
    q = open(os.path.join(QDIR, f"q{i}.rq")).read()
    body = re.search(r"WHERE\s*\{(.*)\}", q, re.S).group(1)
    at = [tuple(t.split()) for t in re.split(r"\s\.\s*", body.strip().rstrip("."))]
    # reorder: constants-bearing atoms first, then connected (so backtracking stays selective); count is order-independent
    def sel(a): return 0 if (not a[0].startswith("?") or not a[2].startswith("?")) else 1
    bound = set(); ordered = []; rest = list(at)
    while rest:
        rest.sort(key=lambda a: (0 if (a[0] in bound or a[2] in bound or (not bound and (not a[0].startswith('?') or not a[2].startswith('?')))) else 1, 0 if (not a[0].startswith('?') or not a[2].startswith('?')) else 1, 0 if a[1] != "rdf:type" else 1))
        a = rest.pop(0); ordered.append(a); bound |= {x for x in (a[0], a[2]) if x.startswith("?")}
    out[i] = solve(ordered); print(i, out[i], flush=True)
json.dump(out, open("truth_py.json", "w"), indent=1)
