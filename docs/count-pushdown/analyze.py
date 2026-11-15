#!/usr/bin/env python3
"""Usage: analyze.py FAMILY BASELINE.tsv CANDIDATE.tsv; requires full cohort parity."""
import sys,csv,json,statistics
from collections import defaultdict
from pathlib import Path
family,base,candidate=sys.argv[1:]
root=Path(__file__).resolve().parent
cohort={p.stem for p in (root/'queries'/family).glob('*.rq')}
refs=json.loads((root/'count_star_references.json').read_text())[family]
def load(path):
    groups=defaultdict(list)
    for r in csv.DictReader(open(path),delimiter='\t'):groups[r['query']].append(r)
    assert set(groups)==cohort,(path,'incomplete cohort')
    for name,rows in groups.items():
        assert len(rows)==5,(path,name,'expected five samples')
        assert len({(r['rows'],r['sha256']) for r in rows})==1,(path,name,'unstable outputs')
        if name in refs: assert all(int(r['count_value'])==refs[name] for r in rows),(name,'external COUNT(*) mismatch')
    return groups
b,c=load(base),load(candidate)
out=[]
for name in sorted(cohort):
    assert (b[name][0]['rows'],b[name][0]['sha256'])==(c[name][0]['rows'],c[name][0]['sha256']),(name,'answer mismatch')
    bt=statistics.median(int(r['query_us']) for r in b[name]);ct=statistics.median(int(r['query_us']) for r in c[name])
    out.append(dict(query=name,rows=int(c[name][0]['rows']),baseline_us=bt,candidate_us=ct,speedup=bt/ct if ct else None))
print(json.dumps(dict(family=family,all_answers_equal=True,queries=len(out),external_count_star_checks=len(refs),baseline_total_us=sum(r['baseline_us'] for r in out),candidate_total_us=sum(r['candidate_us'] for r in out),details=out),indent=2))
