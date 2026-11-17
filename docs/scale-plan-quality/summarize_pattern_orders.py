#!/usr/bin/env python3
"""Require complete bounded arms and exact output parity before computing payoff."""
import ast,csv,json,pathlib,re,statistics
root=pathlib.Path(__file__).parent
summary={}
for dataset,expected in [("bsbm100",{"q010":5,"q013":5}),("watdiv100",{"C3":429276})]:
    run=root/("pattern-"+dataset)
    receipt=json.loads((run/"receipt.json").read_text())
    assert receipt["status"]=="complete" and receipt["exit_code"]==0
    assert receipt["input_unchanged"] and receipt["binary_unchanged"]
    rows=list(csv.DictReader((run/"raw/samples.tsv").open(),delimiter="\t"))
    meta=(run/"raw/metadata.txt").read_text()
    for name,count in expected.items():
        line=next(x for x in meta.splitlines() if x.startswith(f"query={name} ") and "tested=" in x)
        chosen=ast.literal_eval(re.search(r"chosen=(\[[^]]*\])",line).group(1))
        chosen_key=",".join(map(str,chosen))
        n=int(re.search(r"tested=(\d+)",line).group(1))
        q=[r for r in rows if r["query"]==name]
        assert len(q)==7*(n+1)
        assert {int(r["rows"]) for r in q}=={count}
        assert len({r["id_multiset_sha256"] for r in q})==1
        fixed=[r for r in q if r["mode"]=="fixed"]
        normal=[r for r in q if r["mode"]=="normal"]
        assert len(normal)==7 and sum(r["warmup"]=="true" for r in normal)==1
        keys=sorted({r["order"] for r in fixed})
        assert len(keys)==n
        orders={}
        for key in keys:
            order=list(map(int,key.split(",")))
            assert sorted(order)==list(range(len(chosen)))
            s=[r for r in fixed if r["order"]==key]
            assert len(s)==7 and sum(r["warmup"]=="true" for r in s)==1
            values=[int(r["evaluation_ns"])/1e6 for r in s if r["warmup"]=="false"]
            assert len(values)==6
            assert {r["chosen"] for r in s}==({"true"} if key==chosen_key else {"false"})
            orders[key]={"all_ms":values,"median_ms":statistics.median(values),"min_ms":min(values),"max_ms":max(values)}
        best=min(orders,key=lambda k:orders[k]["median_ms"])
        speedup=orders[chosen_key]["median_ms"]/orders[best]["median_ms"]
        controls=[int(r["evaluation_ns"])/1e6 for r in normal if r["warmup"]=="false"]
        summary[name]={"dataset":dataset,"candidate_count":n,"chosen":chosen_key,"best":best,
            "selected_over_best":speedup,"meets_1_5_threshold":speedup>=1.5,
            "normal_control_all_ms":controls,"normal_control_median_ms":statistics.median(controls),
            "orders":orders,"rows":count,"id_multiset_sha256":q[0]["id_multiset_sha256"]}
(root/"pattern-summary.json").write_text(json.dumps(summary,indent=2)+"\n")
for q,s in summary.items():
    print(q,s["candidate_count"],"chosen",round(s["orders"][s["chosen"]]["median_ms"],3),
        "best",round(s["orders"][s["best"]]["median_ms"],3),"speedup",round(s["selected_over_best"],4),
        "normal",round(s["normal_control_median_ms"],3))
