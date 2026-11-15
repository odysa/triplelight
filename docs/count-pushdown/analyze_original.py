#!/usr/bin/env python3
import sys,csv,json,statistics
from pathlib import Path
family,base,candidate=sys.argv[1:]
root=Path(__file__).resolve().parent
refs={k.removeprefix('all_'):v for k,v in json.loads((root/'count_star_references.json').read_text())[family].items()}
def load(path):
    groups={}
    for r in csv.DictReader(open(path),delimiter='\t'):groups.setdefault(r['query'],[]).append(r)
    assert set(groups)==set(refs)
    return groups
b,c=load(base),load(candidate);details=[]
for q in sorted(b):
    assert len(b[q])==len(c[q])==5
    assert len({(r['rows'],r['sha256']) for r in b[q]+c[q]})==1,q
    assert int(b[q][0]['rows'])==refs[q],q
    details.append(dict(query=q,base_us=statistics.median(int(r['query_us']) for r in b[q]),candidate_us=statistics.median(int(r['query_us']) for r in c[q])))
print(json.dumps(dict(equal=True,baseline_total_us=sum(r['base_us'] for r in details),candidate_total_us=sum(r['candidate_us'] for r in details),details=details),indent=2))
