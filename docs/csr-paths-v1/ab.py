import subprocess,time,urllib.request,urllib.parse,os,signal,hashlib,statistics as st
BIN=os.environ["TRIPLELIGHT_BIN"]; STORE=os.environ["LUBM1000_STORE"]
P="PREFIX ub: <http://swat.cse.lehigh.edu/onto/univ-bench.owl#> "
Q={"plus_all":P+"SELECT (COUNT(*) AS ?c) WHERE { ?x ub:subOrganizationOf+ ?y }",
   "star_all":P+"SELECT (COUNT(*) AS ?c) WHERE { ?x ub:subOrganizationOf* ?y }",
   "plus_bound":P+"SELECT ?y WHERE { <http://www.Department0.University0.edu/ResearchGroup0> ub:subOrganizationOf+ ?y }",
   "plus_join":P+"SELECT (COUNT(*) AS ?c) WHERE { ?s ub:memberOf ?d . ?d ub:subOrganizationOf+ ?u }",
   "rev_plus":P+"SELECT (COUNT(*) AS ?c) WHERE { ?u ^ub:subOrganizationOf+ ?x }"}
def req(q):
    r=urllib.request.Request("http://127.0.0.1:7979/sparql",data=urllib.parse.urlencode({"query":q}).encode(),headers={"Accept":"application/sparql-results+json"})
    t=time.perf_counter(); b=urllib.request.urlopen(r,timeout=600).read(); return time.perf_counter()-t, hashlib.sha256(b).hexdigest()
res={}
for i,arm in enumerate(["on","off","off","on"]):
    log=open(f"server-{i}-{arm}.log","w")
    p=subprocess.Popen([BIN,"serve",STORE,"7979","--disk","--cache-mb","6144"]+(["--ablate=csrpaths"] if arm=="off" else []),stdout=log,stderr=log)
    for _ in range(600):
        try: req("ASK{}"); break
        except Exception: time.sleep(0.5)
    time.sleep(25)
    for k,q in Q.items():
        req(q)
        for _ in range(3): s,h=req(q); res.setdefault(k,{}).setdefault(arm,[]).append((s,h))
    p.send_signal(signal.SIGTERM); p.wait()
for k,v in res.items():
    on=st.median([x[0] for x in v['on']]); off=st.median([x[0] for x in v['off']])
    print(k, round(on*1e3,1),"ms on", round(off*1e3,1),"ms off", round(on/off,3), "same" if len(set(x[1] for a in v.values() for x in a))==1 else "DIFFER")
