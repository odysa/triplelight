#!/usr/bin/env python3
"""Prospective value-compatible gate, alongside unchanged strict HTTP records.

The cohort was seen previously. Freeze this adapter/policy before new responses;
only new same-session observations may enter the value-compatible timing table.
"""
import argparse
import hashlib
import json
from pathlib import Path

from envelope_qualify import FrozenInputs
import http_measure
from value_diagnostic import diagnose


def sha(raw): return hashlib.sha256(raw).hexdigest()

def check_policy(path,digest,workload_sha256):
    raw=path.read_bytes()
    if sha(raw)!=digest: raise ValueError('Value ranking policy hash differs')
    policy=json.loads(raw)
    if policy['schema']!='prospective-value-compatible-timing-v1': raise ValueError('Unknown value timing policy')
    if policy['workload_manifest_sha256']!=workload_sha256: raise ValueError('Wrong policy workload')
    for name,h in policy['validator_sources'].items():
        if Path(name).name!=name or sha((Path(__file__).parent/name).read_bytes())!=h:
            raise ValueError('Value policy validator differs: '+name)
    required={'http_measure_value.py','value_diagnostic.py','http_measure.py','qualify.py','envelope_qualify.py','float_envelope.py','workload_check.py','count_equivalence.py'}
    if not required<=set(policy['validator_sources']): raise ValueError('Incomplete value policy source freeze')
    return policy


def run(args):
    manifest_raw=(args.workload/'manifest.json').read_bytes();manifest=json.loads(manifest_raw)
    policy=check_policy(args.value_policy_freeze,args.value_policy_sha256,sha(manifest_raw))
    if (args.numeric_freeze_sha256!=policy['numeric_freeze_sha256'] or
        args.count_policy_sha256!=policy['count_freeze_sha256']): raise ValueError('Original freezes differ')
    frozen=FrozenInputs(args.numeric_inputs,args.numeric_freeze_sha256,manifest['source_data_sha256'],sha(manifest_raw))
    # The original runner retains strict/COUNT records and transport timing.
    http_measure.run(args)
    (args.out/'value-policy-freeze.json').write_bytes(args.value_policy_freeze.read_bytes())
    report_path=args.out/'measurements.json';report=json.loads(report_path.read_text())
    for q in manifest['queries']:
        name=q['file'];stem=Path(name).stem;entry=report['queries'][name]
        detail={'status':'fail'}
        if entry['status']=='not_run':detail={'status':'not_run'}
        elif entry.get('transport_error') or entry.get('http_status')!=200:
            detail={'status':'transport_failure','error':entry.get('error')}
        else:
            raw=(args.out/(stem+'.body')).read_bytes()
            if sha(raw)!=entry['body_sha256']:raise ValueError('Body changed before value gate')
            actual=json.loads(raw);ref_raw=(args.reference/(stem+'.json')).read_bytes();reference=json.loads(ref_raw)
            if sha(ref_raw)!=policy['reference_files'][stem+'.json']:raise ValueError('Reference differs from policy')
            try:
                if q['template']==7:
                    d_raw=(args.reference/(stem+'-diagnostic.json')).read_bytes()
                    if sha(d_raw)!=policy['reference_files'][stem+'-diagnostic.json']:raise ValueError('Cutoff diagnostic differs')
                    result=http_measure.verify(7,actual,reference,json.loads(d_raw),frozen,q)
                    result={'value_bag':'pass','order':{'status':'pass',**result['order']},'rows':result['rows']}
                else:
                    inputs=frozen.for_instance(q) if q['template'] in (4,5,6) else None
                    result=diagnose(q['template'],actual,reference,inputs)
                detail={'status':'pass' if result['order']['status']=='pass' else 'order_failure',**result}
            except (ValueError,KeyError,OverflowError) as exc:
                detail={'status':'fail','error':type(exc).__name__+': '+str(exc)}
        entry['value_compatibility']=detail
    # Binding checks again after scoring prohibit changing a gate mid-session.
    check_policy(args.value_policy_freeze,args.value_policy_sha256,sha(manifest_raw))
    eligible=report['complete'] and len(report['queries'])==len(manifest['queries']) and all(
        v['value_compatibility']['status']=='pass' for v in report['queries'].values())
    report.update(value_policy_sha256=args.value_policy_sha256,value_compatible_eligible=eligible,
                  value_compatible_total_ns=sum(v['elapsed_ns'] for v in report['queries'].values() if v['scored']) if eligible else None)
    report_path.write_text(json.dumps(report,indent=2)+'\n')

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for field in ('workload','reference','out','runtime-evidence','numeric-inputs','count-policy-freeze','value-policy-freeze'):
        parser.add_argument('--'+field,type=Path,required=True)
    for field in ('numeric-freeze-sha256','count-policy-sha256','value-policy-sha256','lock-owner'):
        parser.add_argument('--'+field,required=True)
    parser.add_argument('--timeout',type=float,default=120)
    args=parser.parse_args()
    if args.timeout<=0:parser.error('positive timeout required')
    run(args)
