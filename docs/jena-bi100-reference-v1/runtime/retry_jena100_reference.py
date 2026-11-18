import hashlib,json,sys,time
from pathlib import Path
root=Path(__file__).resolve().parent
sys.path.insert(0,str(root/'jena100-bi-code-v1'))
from workload_check import ask
from qualify import check_order
from envelope_qualify import FrozenInputs
owner='Jianlin-Su task25 retry three 100M Jena BI references'

def collect(endpoint,out):
    plan=json.loads((root/'jena100-reference-retry-plan.json').read_text())
    original=root/'jena100-bi-reference-v1/reference/matrix.json'
    assert hashlib.sha256(original.read_bytes()).hexdigest()==plan['original_matrix_sha256']
    workload=root/'bsbm100-preparation-v1/workload'
    raw=(workload/'manifest.json').read_bytes();manifest=json.loads(raw)
    FrozenInputs(root/'numeric-freeze-100m-v1',
      '63e127b60ac8de510a172748108639f0834e17bc3772e1d16ce226404aa58cca',
      manifest['source_data_sha256'],hashlib.sha256(raw).hexdigest())
    out.mkdir()
    report={'schema':'independent-bi-reference-retry-v1','retry_plan':plan,'queries':{},'complete':False}
    for q in manifest['queries']:
        name=q['file']
        if name not in plan['retry_files']:continue
        assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
        source=(workload/'queries'/name).read_text();assert hashlib.sha256(source.encode()).hexdigest()==q['sha256']
        entry={'template':q['template'],'query_sha256':q['sha256'],'timeout_seconds':600}
        try:
            actual=ask(endpoint,source,600)
            target=out/(Path(name).stem+'.json');target.write_text(json.dumps(actual,indent=2))
            assert q['template'] in (1,3) # Neither has q7 cutoff diagnostics or q4/5/6 anchors.
            entry.update(status='pass',rows=len(actual['results']['bindings']),
                         order=check_order(q['template'],actual),body_sha256=hashlib.sha256(target.read_bytes()).hexdigest())
        except Exception as exc:entry.update(status='fail',error=type(exc).__name__+': '+str(exc))
        report['queries'][name]=entry
        (out/'matrix.json').write_text(json.dumps(report,indent=2))
    report['complete']=len(report['queries'])==3
    report['all_requested_pass']=report['complete'] and all(v['status']=='pass' for v in report['queries'].values())
    (out/'matrix.json').write_text(json.dumps(report,indent=2))
    return report
