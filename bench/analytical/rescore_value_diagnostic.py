#!/usr/bin/env python3
"""Re-score retained BI bodies; no HTTP, servers, timings or policy mutation."""
import argparse
from collections import Counter
import gzip
import hashlib
import json
from pathlib import Path

from envelope_qualify import FrozenInputs
from http_measure import verify
from qualify import q7_allowed_products
from workload_check import diagnostics
from value_diagnostic import diagnose


def sha(raw): return hashlib.sha256(raw).hexdigest()
class Archive:
    def __init__(self,path):
        self.path=path; self.reads={}
        self.manifest_raw=(path/'file-manifest.json').read_bytes()
        self.records={r['path']:r for r in json.loads(self.manifest_raw)}
    def read(self,name):
        item=self.records[name]
        raw=(self.path/item.get('stored_path',name)).read_bytes()
        if 'stored_sha256' in item and sha(raw)!=item['stored_sha256']: raise ValueError('Stored hash differs: '+name)
        if item.get('compression')=='gzip': raw=gzip.decompress(raw)
        if sha(raw)!=item['sha256']: raise ValueError('Raw archive hash differs: '+name)
        self.reads[name]=sha(raw)
        return raw
    def json(self,name): return json.loads(self.read(name))


def run(args):
    out=args.out;out.mkdir(exist_ok=False)
    source_dir=Path(__file__).parent
    policy={'schema':'retrospective-value-diagnostic-policy-v1',
            'retrospective':True,'changes_frozen_qualification':False,
            'files':{n:sha((source_dir/n).read_bytes()) for n in
                     ['value_diagnostic.py','rescore_value_diagnostic.py','test_value_diagnostic.py','http_measure.py']},
            'policy_text_sha256':sha(args.policy.read_bytes())}
    policy_raw=json.dumps(policy,indent=2).encode();(out/'policy-source-freeze.json').write_bytes(policy_raw)
    archive=Archive(args.archive);refs=Archive(args.reference_archive)
    manifest_raw=archive.read('benchmark-source/workloads/bsbm10/manifest.json')
    manifest=json.loads(manifest_raw)
    frozen=FrozenInputs(args.numeric_inputs,'a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244',manifest['source_data_sha256'],sha(manifest_raw))
    summary={'schema':'retrospective-value-diagnostic-summary-v1','policy_sha256':sha(policy_raw),
             'numeric_freeze_sha256':frozen.freeze_sha256,'profiles':{},
             'timing_ranking_created':False,'retrospective':True}
    rows=[]
    for profile in ['rdfdb','duckdb','qlever','qlever-hash','qlever-bypass','virtuoso']:
        for phase in [1,2]:
            name=f'pass{phase}-{profile}';prefix='round/'+name
            m=archive.json(prefix+'/queries/measurements.json')
            counts=Counter();failures=Counter();records=[]
            for q in manifest['queries']:
                file=q['file'];stem=Path(file).stem;entry=m['queries'][file]
                query_raw=archive.read('benchmark-source/workloads/bsbm10/queries/'+file)
                if sha(query_raw)!=q['sha256']: raise ValueError('Query hash differs')
                record={'query':file,'template':q['template'],'mix':q['mix'],
                        'strict_status':entry['status'],
                        'count_status':entry.get('count_equivalence',{}).get('status','not_run')}
                if entry['status']=='not_run': record['value_status']='not_run'
                elif entry.get('transport_error') or entry.get('http_status')!=200:
                    record.update(value_status='transport_failure',error=entry.get('error'))
                else:
                    raw=archive.read(prefix+'/queries/'+stem+'.body')
                    if sha(raw)!=entry['body_sha256']:raise ValueError('Measurement body hash differs')
                    actual=json.loads(raw);ref=refs.json('jena/'+stem+'.json')
                    record.update(body_sha256=sha(raw),reference_sha256=refs.reads['jena/'+stem+'.json'])
                    try:
                        if q['template']==7:
                            diagnostic=refs.json('jena/'+stem+'-diagnostic.json')
                            # Retain original shape, cutoff and uniqueness validation.
                            detail=verify(7,actual,ref,diagnostic,frozen,q)
                            detail={'value_bag':'pass','order':{'status':'pass',**detail['order']},'rows':detail['rows']}
                        else:
                            inputs=frozen.for_instance(q) if q['template'] in (4,5,6) else None
                            detail=diagnose(q['template'],actual,ref,inputs)
                        record.update(detail,value_status='pass' if detail['order']['status']=='pass' else 'order_failure')
                    except (ValueError,KeyError,OverflowError) as exc:
                        record.update(value_status='fail',error=type(exc).__name__+': '+str(exc))
                counts[record['value_status']]+=1
                if record.get('error'): failures[(record['template'],record['error'])]+=1
                records.append(record)
            summary['profiles'][name]={'scheduled':len(records),'strict_pass':sum(r['strict_status']=='pass' for r in records),
                 'count_pass':sum(r['count_status']=='pass' for r in records),
                 'value_status_counts':dict(counts),'value_bag_pass':sum(r.get('value_bag')=='pass' for r in records),
                 'failures':[{'template':k[0],'error':k[1],'instances':v} for k,v in failures.items()]}
            rows.extend(dict(profile=name,**r) for r in records)
    # Compact wrong-bag case, retaining raw query/count and actual/reference JSON.
    q=next(q for q in manifest['queries'] if q['template']==5)
    stem=Path(q['file']).stem; proof=out/'virtuoso-q5';proof.mkdir()
    evidence={'query.rq':archive.read('benchmark-source/workloads/bsbm10/queries/'+q['file']),
              'actual.json':archive.read('round/pass1-virtuoso/queries/'+stem+'.body'),
              'reference.json':refs.read('jena/'+stem+'.json'),
              'graph-count.json':archive.read('round/pass1-virtuoso/graph-count.json')}
    for n,raw in evidence.items(): (proof/n).write_bytes(raw)
    summary['virtuoso_q5_example']={'query':q['file'],'query_sha256':q['sha256'],
          'actual_rows':len(json.loads(evidence['actual.json'])['results']['bindings']),
          'reference_rows':len(json.loads(evidence['reference.json'])['results']['bindings']),
          'files':{n:sha(raw) for n,raw in evidence.items()}}
    (out/'requests.json').write_text(json.dumps(rows,indent=2)+'\n')
    (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    (out/'input-identities.json').write_text(json.dumps({'archive_manifest_sha256':sha(archive.manifest_raw),
          'reference_manifest_sha256':sha(refs.manifest_raw),'archive_files_verified':archive.reads,
          'reference_files_verified':refs.reads},indent=2)+'\n')
    print(json.dumps(summary,indent=2))

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for n in ['archive','reference-archive','numeric-inputs','policy','out']:
        parser.add_argument('--'+n,type=Path,required=True)
    run(parser.parse_args())
