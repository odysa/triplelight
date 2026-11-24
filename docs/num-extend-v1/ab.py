import subprocess,hashlib,re,statistics as st,collections
import os; S=os.environ["BSBM100_STORE"]
QD=os.environ["BI_QUERIES"]+"/"
Q=["m000_p01_q3","m001_p01_q3","m002_p01_q3","m003_p01_q3","m000_p02_q2","m000_p03_q2","m001_p02_q2","m002_p02_q2"]
B={"main":os.environ["TL_MAIN"],"cand":os.environ["TL_CAND"]}
d=collections.defaultdict(lambda: collections.defaultdict(list)); h=collections.defaultdict(set)
for arm in ["main","cand","cand","main"]:
    for q in Q:
        text=open(QD+q+".rq").read()
        for i in range(4):
            r=subprocess.run([B[arm],"query",S,text,"--disk"],capture_output=True)
            ms=float(re.search(rb"query ([0-9.]+) ms",r.stderr).group(1)); h[q].add(hashlib.sha256(r.stdout).hexdigest()[:12])
            if i>0: d[q][arm].append(ms)
for q in Q:
    m=st.median(d[q]['main']); c=st.median(d[q]['cand'])
    print(f"{q} main {m:.1f} cand {c:.1f} ratio {c/m:.3f} {'same' if len(h[q])==1 else 'DIFFER'}")
