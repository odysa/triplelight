#!/usr/bin/env python3
"""Audit the completed two-pass HTTP cohort; never total an unqualified profile."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import re


def resource(path):
    if not path.exists():return None
    text=path.read_text();result={'scope':'whole server lifetime, including startup/readiness/warmup and measured queries'}
    for key in ('VmHWM','VmRSS','VmPeak'):
        match=re.search(r'^'+key+r':\s+(\d+)\s+kB',text,re.M)
        result[key+'_bytes']=int(match[1])*1024 if match else None
    raw=[int(line) for line in text.splitlines() if re.fullmatch(r'\d+',line)]
    result['cgroup_memory_peak_bytes']=raw[0] if len(raw)>=2 else None
    result['cgroup_memory_limit_bytes']=raw[1] if len(raw)>=2 else None
    return result


def summarize(run):
    schedule=json.loads((run/'schedule.json').read_text());statuses=json.loads((run/'status.json').read_text())
    names=[f'pass{index}-'+mode for index,modes in enumerate(schedule['passes'],1) for mode in modes]
    if set(statuses)!=set(names):raise ValueError('The scheduled phases have not all finished')
    report={'schema':'matched-bi-http-summary-v1','schedule':schedule,'profiles':{},
            'official_bsbm_throughput_score':False,'complete_cross_engine_ranking':False}
    for name in names:
        phase=run/name;matrix_path=phase/'queries/measurements.json';entry={'controller':statuses[name]}
        report['profiles'][name]=entry
        if not matrix_path.exists():
            entry.update(complete=False,strict_eligible=False,count_equivalent_eligible=False,
                         measured_total_ms=None,count_equivalent_total_ms=None)
            continue
        matrix=json.loads(matrix_path.read_text());manifest=json.loads((phase/'queries/manifest.json').read_text())
        expected={q['file']:q for q in manifest['queries']};queries=matrix['queries']
        entry['complete']=matrix['complete'] and set(queries)==set(expected)
        entry['requested']=len(expected);entry['attempted']=sum(q['status']!='not_run' for q in queries.values())
        entry['strict_pass']=sum(q['status']=='pass' for q in queries.values())
        entry['count_equivalent_pass']=sum(q.get('count_equivalence',{}).get('status')=='pass' for q in queries.values())
        failures={'strict':Counter(),'count_equivalent':Counter()}
        for file,q in queries.items():
            if q['mix']!=expected[file]['mix'] or q['template']!=expected[file]['template']:
                raise ValueError('Scheduled query metadata differs: '+name+'/'+file)
            if q.get('body_sha256'):
                raw=(phase/'queries'/(Path(file).stem+'.body')).read_bytes()
                if hashlib.sha256(raw).hexdigest()!=q['body_sha256']:raise ValueError('Raw response hash differs')
            if q['status']!='pass':failures['strict'][(q['template'],q.get('error',q['status']))]+=1
            value=q.get('count_equivalence',{})
            if value.get('status')!='pass':failures['count_equivalent'][(q['template'],value.get('error',q.get('error',q['status'])))]+=1
        entry['failures']={policy:[{'template':key[0],'error':key[1],'instances':count} for key,count in counts.items()]
                           for policy,counts in failures.items()}
        strict=entry['complete'] and entry['strict_pass']==len(expected)
        equivalent=entry['complete'] and entry['count_equivalent_pass']==len(expected)
        if bool(matrix['ranking_eligible'])!=strict or bool(matrix.get('integer_value_equivalent_eligible'))!=equivalent:
            raise ValueError('Eligibility flags contradict complete qualified coverage: '+name)
        entry['strict_eligible']=strict;entry['count_equivalent_eligible']=equivalent
        measured=[q for q in queries.values() if q['mix'] in schedule['scored_mixes']]
        if entry['complete'] and len(measured)!=45:raise ValueError('Scored mix coverage differs')
        entry['measured_requests']=len(measured)
        total=sum(q['elapsed_ns'] for q in measured) if strict or equivalent else None
        if strict and matrix['measured_total_ns']!=total:raise ValueError('Strict total differs')
        if equivalent and matrix['integer_value_equivalent_total_ns']!=total:raise ValueError('Equivalent total differs')
        entry['measured_total_ms']=total/1e6 if strict else None
        entry['count_equivalent_total_ms']=total/1e6 if equivalent else None
        entry['qualified_template_totals_ms']=({str(n):sum(q['elapsed_ns'] for q in measured if q['template']==n)/1e6
                                              for n in range(1,9)} if strict else None)
        entry['resource']=resource(phase/'after.resource.txt')
        temp=phase/'temp-observation.json';entry['named_temp_observation']=json.loads(temp.read_text()) if temp.exists() else None
        entry['runtime_evidence_sha256']=matrix['runtime_evidence_sha256']
        entry['numeric_freeze_sha256']=matrix['numeric_freeze_sha256']
        entry['count_equivalence_freeze_sha256']=matrix['count_equivalence_freeze_sha256']
    primary=[report['profiles'][f'pass{p}-'+mode] for p in (1,2) for mode in ('rdfdb','duckdb','virtuoso','qlever')]
    report['all_primary_profiles_strictly_qualify']=all(p['strict_eligible'] for p in primary)
    report['all_primary_profiles_count_equivalent_qualify']=all(p['count_equivalent_eligible'] for p in primary)
    report['complete_cross_engine_ranking']=report['all_primary_profiles_count_equivalent_qualify']
    return report


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--run',type=Path,required=True);parser.add_argument('--out',type=Path,required=True)
    args=parser.parse_args()
    if args.out.exists():raise FileExistsError('Preserve prior summaries')
    report=summarize(args.run);args.out.write_text(json.dumps(report,indent=2))
    for name,profile in report['profiles'].items():
        print(name,profile.get('strict_pass'),profile.get('count_equivalent_pass'),profile['measured_total_ms'],profile['count_equivalent_total_ms'])
