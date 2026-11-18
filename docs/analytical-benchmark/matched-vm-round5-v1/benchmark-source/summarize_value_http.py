#!/usr/bin/env python3
"""Audit prospective value-compatible timings without changing strict totals."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path

from envelope_qualify import FrozenInputs
from http_measure import verify
from http_measure_value import check_policy
from summarize_http import summarize
from value_diagnostic import diagnose


def sha(raw): return hashlib.sha256(raw).hexdigest()

def audit(run,reference,numeric_inputs,policy_path,policy_sha):
    result=summarize(run,('rdfdb','duckdb','qlever'))
    result['schema']='matched-bi-three-value-http-summary-v1'
    result['value_policy_sha256']=policy_sha
    result['value_policy_origin']='Retrospective numeric rule frozen before this new same-session timing cohort; previously seen workload'
    for name,entry in result['profiles'].items():
        phase=run/name;matrix=json.loads((phase/'queries/measurements.json').read_text())
        manifest_raw=(phase/'queries/manifest.json').read_bytes();manifest=json.loads(manifest_raw)
        policy=check_policy(policy_path,policy_sha,sha(manifest_raw))
        if matrix['value_policy_sha256']!=policy_sha:raise ValueError('Phase policy differs')
        frozen=FrozenInputs(numeric_inputs,policy['numeric_freeze_sha256'],manifest['source_data_sha256'],sha(manifest_raw))
        evidence_raw=(phase/'runtime-evidence.json').read_bytes();evidence=json.loads(evidence_raw)
        if sha(evidence_raw)!=matrix['runtime_evidence_sha256']:raise ValueError('Runtime evidence hash differs')
        statuses=Counter();failures=Counter()
        for q in manifest['queries']:
            file=q['file'];stem=Path(file).stem;v=matrix['queries'][file]
            if v['scored']!=(q['mix'] in (1,2,3)):raise ValueError('Measured request selection differs')
            if v['status']=='not_run':status='not_run'
            elif v.get('transport_error') or v.get('http_status')!=200:status='transport_failure'
            else:
                raw=(phase/'queries'/(stem+'.body')).read_bytes()
                if sha(raw)!=v['body_sha256']:raise ValueError('Body hash differs')
                ref_raw=(reference/(stem+'.json')).read_bytes()
                if sha(ref_raw)!=policy['reference_files'][stem+'.json']:raise ValueError('Reference hash differs')
                actual=json.loads(raw);ref=json.loads(ref_raw)
                try:
                    if q['template']==7:
                        d_raw=(reference/(stem+'-diagnostic.json')).read_bytes()
                        if sha(d_raw)!=policy['reference_files'][stem+'-diagnostic.json']:raise ValueError('Cutoff hash differs')
                        detail=verify(7,actual,ref,json.loads(d_raw),frozen,q)
                        detail={'value_bag':'pass','order':{'status':'pass',**detail['order']},'rows':detail['rows']}
                    else:
                        inputs=frozen.for_instance(q) if q['template'] in (4,5,6) else None
                        detail=diagnose(q['template'],actual,ref,inputs)
                    status='pass' if detail['order']['status']=='pass' else 'order_failure'
                    if v['value_compatibility']!={'status':status,**detail}:raise RuntimeError('Value detail differs from independent replay')
                except (ValueError,KeyError,OverflowError) as exc:
                    status='fail';failures[(q['template'],type(exc).__name__+': '+str(exc))]+=1
                if name.endswith('qlever'):
                    control=v.get('cache_control',{})
                    if control.get('http_status')!=200 or control.get('transport_error'):raise ValueError('QLever cache clear failed')
                    reply=json.loads((phase/'queries'/(stem+'-control.body')).read_bytes())
                    expected=evidence['before_each_request']['expected_json']
                    if reply!=expected or not reply or any(value!=0 for value in reply.values()):raise ValueError('QLever cache clear acknowledgement differs')
            if status!=v['value_compatibility']['status']:raise ValueError('Value outcome differs from replay')
            statuses[status]+=1
        eligible=entry['complete'] and statuses['pass']==60
        total=sum(v['elapsed_ns'] for v in matrix['queries'].values() if v['mix'] in (1,2,3)) if eligible else None
        if bool(matrix['value_compatible_eligible'])!=eligible or matrix['value_compatible_total_ns']!=total:
            raise ValueError('Value eligibility or total differs')
        entry.update(value_status_counts=dict(statuses),value_compatible_eligible=eligible,
                     value_compatible_total_ms=total/1e6 if total is not None else None,
                     value_compatible_template_ms={str(n):sum(v['elapsed_ns'] for v in matrix['queries'].values() if v['mix'] in (1,2,3) and v['template']==n)/1e6 for n in range(1,9)} if eligible else None,
                     value_failures=[{'template':k[0],'error':k[1],'instances':count} for k,count in failures.items()])
    result['complete_value_compatible_three_engine_ranking']=all(e['value_compatible_eligible'] for e in result['profiles'].values())
    return result

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    for n in ['run','reference','numeric-inputs','policy','out']:p.add_argument('--'+n,type=Path,required=True)
    p.add_argument('--policy-sha256',required=True);a=p.parse_args()
    if a.out.exists():raise FileExistsError('Preserve summaries')
    result=audit(a.run,a.reference,a.numeric_inputs,a.policy,a.policy_sha256)
    a.out.write_text(json.dumps(result,indent=2)+'\n')
    for name,v in result['profiles'].items():print(name,v['value_status_counts'],v['value_compatible_total_ms'])
