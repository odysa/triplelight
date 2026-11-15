#!/usr/bin/env python3
"""Recompute all summaries from the original, unedited sampling_probe TSVs."""
import csv,json,statistics,math
from collections import defaultdict
from pathlib import Path
ROOT=Path(__file__).resolve().parent
summary={}
for dataset in ('lubm100','watdiv100'):
    rows=list(csv.DictReader((ROOT/f'{dataset}.tsv').open(),delimiter='\t'))
    groups=defaultdict(list)
    for row in rows: groups[row['query']].append(row)
    ref=json.loads((ROOT/'reference_counts.json').read_text())[dataset]
    details=[]
    for query,rs in groups.items():
        actual=int(rs[0]['actual'])
        assert all(int(r['actual'])==actual for r in rs)
        assert actual==ref[query],(query,actual)
        med=lambda field:statistics.median(float(r[field]) for r in rs)
        details.append(dict(query=query,actual=actual,static_qerror=float(rs[0]['static_qerror']),sample_median_qerror=med('sample_qerror'),sample_median_us=med('sample_us'),zero_hit_seeds=sum(int(r['successes'])==0 for r in rs),gated_complete=sum(r['gate_complete']=='true' for r in rs),wcoj_off_median_us=med('binary_us'),wcoj_on_median_us=med('leapfrog_us')))
    positives=[d for d in details if d['actual']>0]
    summary[dataset]=dict(queries=len(groups),samples=len(rows),gated_complete=sum(r['gate_complete']=='true' for r in rows),positive_queries_with_any_false_zero=sum(d['zero_hit_seeds']>0 for d in positives),sample_total_median_us=sum(d['sample_median_us'] for d in details),wcoj_off_total_median_us=sum(d['wcoj_off_median_us'] for d in details),wcoj_on_total_median_us=sum(d['wcoj_on_median_us'] for d in details),details=details)
def finite(obj):
    if isinstance(obj,float) and not math.isfinite(obj):return 'infinity'
    if isinstance(obj,dict):return {k:finite(v) for k,v in obj.items()}
    if isinstance(obj,list):return [finite(v) for v in obj]
    return obj
print(json.dumps(finite(summary),indent=2,allow_nan=False))
