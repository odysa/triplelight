# Bounds for #99 as registered in the task thread (rule fixed before statistics were computed).
import json, re, sys, math, glob
st = json.load(open("stats.json")); P = st["preds"]; CC = st["class_counts"]; K = st["const"]
UB = "http://swat.cse.lehigh.edu/onto/univ-bench.owl#"; RDFT = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
def pred(name): return P["<" + (RDFT if name == "rdf:type" else UB + name.split(":")[1]) + ">"]
def parse(q):
    body = re.search(r"WHERE\s*\{(.*)\}", q, re.S).group(1)
    atoms = []
    for t in re.split(r"\s\.\s*", body.strip().rstrip(".")):
        s, p, o = t.split()
        atoms.append((s, p, o))
    return atoms
isv = lambda x: x.startswith("?")
def const_cnt(kind, p, c):
    return K[f"{kind}|{p.split(':')[1]}|{c}"]
def card_and_factor(a, B):
    s, p, o = a; ps = pred(p) if p != "rdf:type" else None
    if p == "rdf:type":
        n = CC["<" + UB + o.split(":")[1] + ">"]
        # subject var; object is a class constant
        return ("ctype", n, 1 if (s in B) else n)
    S = isv(s) and s not in B; O = isv(o) and o not in B   # unbound variable ends
    if not S and not O: return ("full", None, 1)
    if S and not O:    # s unbound
        if not isv(o): n = const_cnt("o", p, o); return ("cst", n, n)
        return ("var", ps["N"], ps["Do"])
    if O and not S:
        if not isv(s): n = const_cnt("s", p, s); return ("cst", n, n)
        return ("var", ps["N"], ps["Ds"])
    return ("free", ps["N"], ps["N"])
def run(atoms):
    B = set(); rest = list(range(len(atoms))); order = []; factors = []
    first = True
    while rest:
        cand = []
        for i in rest:
            a = atoms[i]; kind, card, fac = card_and_factor(a, B)
            vs = {x for x in (a[0], a[2]) if isv(x)}
            if first: key = (card if card is not None else 1, i)
            else:
                if not (vs & B) and kind not in ("full",): key = (1, card if card is not None else 1, i)  # cross product last
                else: key = (0, fac, i)
            cand.append((key, i, fac, card, kind))
        cand.sort(); _, i, fac, card, kind = cand[0]
        order.append(i); factors.append(card if first else fac); first = False
        B |= {x for x in (atoms[i][0], atoms[i][2]) if isv(x)}; rest.remove(i)
    return order, factors
def l2_first(atoms, order):
    # l2 bound for the first join: |R join_v S| <= ||deg_R(v)||_2 * ||deg_S(v)||_2; only when both atoms are plain
    # variable-predicate atoms (non-type) joined on one variable; otherwise no gain (returns None)
    if len(order) < 2: return None
    a, b = atoms[order[0]], atoms[order[1]]
    shared = [x for x in (a[0], a[2]) if isv(x) and x in (b[0], b[2])]
    if len(shared) != 1 or a[1] == "rdf:type" or b[1] == "rdf:type": return None
    if not (isv(a[0]) and isv(a[2]) and isv(b[0]) and isv(b[2])): return None
    v = shared[0]
    na = pred(a[1])["l2s" if a[0] == v else "l2o"]; nb = pred(b[1])["l2s" if b[0] == v else "l2o"]
    return na * nb
rows = []
for i in range(1, 15):
    q = open(f"/private/tmp/wxp-tl/triplelight/bench/lubm/q{i}.rq").read()
    at = parse(q); order, fac = run(at)
    a = 1
    for f in fac: a *= f
    l2 = l2_first(at, order)
    b = None
    if l2 is not None:
        b = l2
        for f in fac[2:]: b *= f
    best = min(a, b) if b is not None else a
    rows.append({"q": i, "order": [list(at[j]) for j in order], "factors": fac, "bound_a": a, "bound_l2": b, "bound": best})
json.dump(rows, open("bounds.json", "w"), indent=1)
for r in rows: print(r["q"], r["bound_a"], r["bound_l2"], r["bound"], [x[1] for x in r["order"]])
