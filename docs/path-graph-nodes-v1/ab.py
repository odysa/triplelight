import subprocess,hashlib,re,statistics as st,collections,sys
import os; S=os.environ["LUBM1000_STORE"]
P="PREFIX ub: <http://swat.cse.lehigh.edu/onto/univ-bench.owl#> "
Q={"star":"SELECT (COUNT(*) AS ?c) WHERE { ?x ub:subOrganizationOf* ?y }",
 "plus":"SELECT (COUNT(*) AS ?c) WHERE { ?x ub:subOrganizationOf+ ?y }",
 "rplus":"SELECT (COUNT(*) AS ?c) WHERE { ?u ^ub:subOrganizationOf+ ?x }",
 "bound":"SELECT ?y WHERE { <http://www.Department0.University0.edu/ResearchGroup0> ub:subOrganizationOf+ ?y }",
 "ord0":"SELECT ?x ?y WHERE { ?x ub:subOrganizationOf* ?y } LIMIT 300000",
 "ord1":"SELECT ?x ?y WHERE { ?x ub:subOrganizationOf* ?y } OFFSET 21700000 LIMIT 300000",
 "ord2":"SELECT ?x ?y WHERE { ?x ub:subOrganizationOf* ?y } OFFSET 33300000 LIMIT 300000"}
d=collections.defaultdict(lambda: collections.defaultdict(list)); h=collections.defaultdict(set)
for arm in ["main","cand","cand","main"]:
    for k,q in Q.items():
        reps=1 if k.startswith("ord") else 4
        for i in range(reps):
            r=subprocess.run([os.environ["TL_"+arm.upper()],"query",S,P+q,"--disk"],capture_output=True)
            ms=float(re.search(rb"query ([0-9.]+) ms",r.stderr).group(1)); sh=hashlib.sha256(r.stdout).hexdigest()[:12]
            h[k].add(sh)
            if i>0 or reps==1: d[k][arm].append(ms)
            print(arm,k,ms,sh,flush=True)
for k in Q:
    m=st.median(d[k]['main']); c=st.median(d[k]['cand'])
    print(f"SUMMARY {k:6} main {m:9.1f} cand {c:9.1f} ratio {c/m:.3f} {'same' if len(h[k])==1 else 'DIFFER '+str(h[k])}")
