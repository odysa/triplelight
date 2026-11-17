#!/usr/bin/env python3
"""Static inventory of parser-exported BGP triples, with strict token/cohort checks."""
import csv,json,pathlib,re
ROOT=pathlib.Path(__file__).parent
TERM=re.compile(r'<[^>]*>|"(?:[^"\\]|\\.)*"(?:@[A-Za-z-]+|\^\^<[^>]*>)?|[?$][A-Za-z_][A-Za-z0-9_]*|_:[A-Za-z0-9_]+')
def cyclic(edges):
    edges=[set(e) for e in edges]
    while True:
        changed=False
        counts={v:sum(v in e for e in edges) for e in edges for v in e}
        for e in edges:
            old=len(e);e.intersection_update(v for v in tuple(e) if counts[v]>1);changed|=old!=len(e)
        keep=[True]*len(edges)
        for i,e in enumerate(edges):
            if not e:keep[i]=False;continue
            if any(j!=i and keep[j] and e<=other and (e!=other or j<i) for j,other in enumerate(edges)):
                keep[i]=False
        updated=[e for e,k in zip(edges,keep) if k]
        changed|=len(updated)!=len(edges);edges=updated
        if not edges:return False
        if not changed:return True
all_rows=[]
for cohort in ["watdiv","bsbm100"]:
    root=ROOT/(cohort+"-probes")
    for row in csv.DictReader((root/"manifest.tsv").open(),delimiter="\t"):
        text=(root/"select"/(row["probe"]+".rq")).read_text()
        body=text[text.index("{")+1:text.rindex("}")]
        tokens=TERM.findall(body)
        residue=TERM.sub("",body).replace(".","").strip()
        if residue or len(tokens)!=3*int(row["patterns"]):
            raise RuntimeError(f"Unexpected exported syntax: {row['probe']} residue={residue!r}")
        edges=[];repeated=False
        for i in range(0,len(tokens),3):
            vs=[t for t in tokens[i:i+3] if t.startswith(("?","$","_:"))]
            repeated|=len(set(vs))!=len(vs);edges.append(set(vs))
        vs=set().union(*edges) if edges else set()
        cy=cyclic(edges)
        # Independent forest test for these cohorts' unary/binary hyperedges.
        if any(len(e)>2 for e in edges):
            raise RuntimeError("Forest cross-check requires at most two variables per triple")
        parent={v:v for v in vs}
        def find(v):
            while parent[v]!=v:
                parent[v]=parent[parent[v]];v=parent[v]
            return v
        graph_cycle=False
        for u,v in {tuple(sorted(e)) for e in edges if len(e)==2}:
            a,b=find(u),find(v)
            if a==b:
                graph_cycle=True;break
            parent[a]=b
        assert graph_cycle==cy, "Independent cycle checks disagree"
        all_rows.append({**row,"cohort":cohort,"variables":len(vs),"cyclic":cy,
            "no_repeated_variable_triples":not repeated,
            "potential_leapfrog":int(row["patterns"])>=3 and cy and not repeated,
            "context_note":"Eligibility also requires one completely unbound input row"})
(ROOT/"bgp-shapes.json").write_text(json.dumps(all_rows,indent=2)+"\n")
for c in ["watdiv","bsbm100"]:
    rows=[r for r in all_rows if r["cohort"]==c]
    print(c,"BGPs",len(rows),"potential leapfrog",sum(r["potential_leapfrog"] for r in rows),
        "cyclic",sum(r["cyclic"] for r in rows))
    for r in rows:
        if r["source"] in ["C3","q010"]:
            print(r["probe"],"variables",r["variables"],"patterns",r["patterns"],"cyclic",r["cyclic"])
