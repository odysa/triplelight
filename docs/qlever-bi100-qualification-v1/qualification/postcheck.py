import collections,hashlib,json,sys
from pathlib import Path
stage=Path(__file__).resolve().parent;root=stage.parent
sys.path.insert(0,str(stage/'bench-source'))
from http_measure import verify
from http_measure_value import check_policy
from envelope_qualify import FrozenInputs
from value_diagnostic import diagnose
from count_equivalence import normalize,load_freeze
from qualify import exact_term
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
workload=root/'bsbm100-preparation-v1/workload';ref=root/'bsbm100-reference-v1'
manifest=json.loads((workload/'manifest.json').read_text())
policy=check_policy(ref/'value-policy-freeze.json','7b3f640bbab516db80ceba87549b9670802a93c5ff77181bdda66c0855fc3dfb',sha(workload/'manifest.json'))
load_freeze(ref/'count-policy-freeze.json','5049fb75210fc6fc78ebf1fb2553b88baef197ae73495b70f110a203a70a4663')
frozen=FrozenInputs(root/'numeric-freeze-100m-v1','63e127b60ac8de510a172748108639f0834e17bc3772e1d16ce226404aa58cca',manifest['source_data_sha256'],sha(workload/'manifest.json'))
report=json.loads((stage/'queries/measurements.json').read_text())
assert report['complete'] and len(report['queries'])==60
out={'queries':{},'comparative_timing_claim':False}
def succeeds(fn):
    try:fn();return True
    except (ValueError,KeyError,OverflowError):return False
def bag(result):return collections.Counter(tuple((k,exact_term(v)) for k,v in sorted(row.items())) for row in result['results']['bindings'])
for q in manifest['queries']:
    name=q['file'];n=q['template'];stem=Path(name).stem;entry=report['queries'][name]
    body=stage/'queries'/(stem+'.body');expected=ref/'reference'/(stem+'.json')
    assert sha(workload/'queries'/name)==q['sha256'] and sha(body)==entry['body_sha256']
    assert entry['http_status']==200 and not entry['transport_error']
    assert sha(expected)==policy['reference_files'][expected.name]
    actual=json.loads(body.read_bytes());reference=json.loads(expected.read_bytes())
    diagnostic_path=ref/'reference'/(stem+'-diagnostic.json')
    d=json.loads(diagnostic_path.read_text()) if n==7 else None
    if n==7:assert sha(diagnostic_path)==policy['reference_files'][diagnostic_path.name]
    primary=succeeds(lambda:verify(n,actual,reference,d,frozen,q))
    count=succeeds(lambda:verify(n,normalize(n,actual),normalize(n,reference),d,frozen,q))
    def value_check():
        if n==7:verify(n,actual,reference,d,frozen,q)
        else:
            r=diagnose(n,actual,reference,frozen.for_instance(q) if n in (4,5,6) else None)
            if r['order']['status']!='pass':raise ValueError('Value order failure')
    value=succeeds(value_check)
    assert primary==(entry['status']=='pass')
    assert count==(entry.get('count_equivalence',{}).get('status')=='pass')
    assert value==(entry.get('value_compatibility',{}).get('status')=='pass')
    control=stage/'queries'/(stem+'-control.body')
    assert json.loads(control.read_bytes())==json.loads((stage/'initial-clear.json').read_text())
    out['queries'][name]={'primary':primary,'count':count,'value':value,'exact_term_bag':bag(actual)==bag(reference),
       'body_sha256':sha(body),'reference_sha256':sha(expected),'cache_control_sha256':sha(control)}
out['counts']={key:sum(v[key] for v in out['queries'].values()) for key in ['primary','count','value','exact_term_bag']}
for name,digest in json.loads((stage/'bench-source/source-sha256.json').read_text()).items():assert sha(stage/'bench-source'/name)==digest
assert json.loads((stage/'index-after.json').read_text())==json.loads((root/'qlever100-build-v1/status.json').read_text())['index_files']
out['source_index_and_cache_acknowledgements_verified']=True
(stage/'postchecks.json').write_text(json.dumps(out,indent=2))
print(out['counts'])
