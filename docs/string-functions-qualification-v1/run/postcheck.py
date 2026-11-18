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
def rows(result):
    return [tuple((k,exact_term(v)) for k,v in sorted(row.items())) for row in result['results']['bindings']]
report=json.loads((stage/'queries/measurements.json').read_text())
out={'queries':{},'comparative_timing_claim':False}
for q in manifest['queries']:
    n=q['template'];stem=Path(q['file']).stem
    body=stage/'queries'/(stem+'.body');expected=ref/'reference'/(stem+'.json')
    assert sha(workload/'queries'/q['file'])==q['sha256']
    assert sha(body)==report['queries'][q['file']]['body_sha256']
    assert sha(expected)==policy['reference_files'][expected.name]
    actual=json.loads(body.read_bytes());reference=json.loads(expected.read_bytes())
    baseline=json.loads((root/'bi100-rdfdb-qualification-v1/queries'/(stem+'.body')).read_bytes())
    diagnostic=json.loads((ref/'reference'/(stem+'-diagnostic.json')).read_text()) if n==7 else None
    verify(n,actual,reference,diagnostic,frozen,q)
    verify(n,normalize(n,actual),normalize(n,reference),diagnostic,frozen,q)
    if n!=7:
        result=diagnose(n,actual,reference,frozen.for_instance(q) if n in (4,5,6) else None)
        assert result['order']['status']=='pass'
    same_bag=collections.Counter(rows(actual))==collections.Counter(rows(baseline))
    same_sequence=rows(actual)==rows(baseline)
    out['queries'][q['file']]={'primary':True,'count':True,'value':True,
      'baseline_exact_bag':same_bag,'baseline_exact_sequence':same_sequence,
      'body_sha256':sha(body),'reference_sha256':sha(expected),
      'baseline_body_sha256':sha(root/'bi100-rdfdb-qualification-v1/queries'/(stem+'.body'))}
out['counts']={key:sum(v[key] for v in out['queries'].values()) for key in ['primary','count','value','baseline_exact_bag','baseline_exact_sequence']}
for name,digest in json.loads((stage/'bench-source/source-sha256.json').read_text()).items():assert sha(stage/'bench-source'/name)==digest
for name,digest in json.loads((stage/'source-identity.json').read_text())['files'].items():assert sha(stage/'source'/name)==digest
assert sha(stage/'rdfdb-ef7a778')==(stage/'binary-sha256.txt').read_text().strip()
assert json.loads((stage/'store-before.json').read_text())==json.loads((stage/'store-after.json').read_text())
fixtures=json.loads((stage/'fixture-results.json').read_text());assert fixtures['complete'] and fixtures['all_pass']
out['fixture_cases']=len(fixtures['cases']);out['sources_binary_snapshot_unchanged']=True
(stage/'postchecks.json').write_text(json.dumps(out,indent=2))
print(out['counts'], 'fixtures',out['fixture_cases'])
