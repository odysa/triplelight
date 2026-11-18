import hashlib,json,sys
from pathlib import Path
root=Path(__file__).resolve().parent
sys.path.insert(0,str(root/'jena100-bi-code-v1'))
from workload_check import ask
from qualify import check_order,qualify
owner='Jianlin-Su task25 Jena q1 original-order planning check'

def collect(endpoint,out):
    workload=root/'bsbm100-preparation-v1/workload'
    manifest=json.loads((workload/'manifest.json').read_text())
    records={q['file']:q for q in manifest['queries']}
    names=['m002_p00_q1.rq','m000_p00_q1.rq','m001_p00_q1.rq','m003_p00_q1.rq']
    out.mkdir()
    report={'schema':'independent-q1-no-reorder-v1','queries':{},'complete':False,
      'query_texts_unchanged':True,'context':{'arq:optReorderBGP':False}}
    for name in names:
        assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
        q=records[name];text=(workload/'queries'/name).read_text()
        assert hashlib.sha256(text.encode()).hexdigest()==q['sha256']
        entry={'query_sha256':q['sha256'],'timeout_seconds':300}
        try:
            actual=ask(endpoint,text,300)
            target=out/(Path(name).stem+'.json');target.write_text(json.dumps(actual,indent=2))
            entry.update(status='pass',rows=len(actual['results']['bindings']),
              order=check_order(1,actual),body_sha256=hashlib.sha256(target.read_bytes()).hexdigest())
            if name!='m002_p00_q1.rq':
                source=root/('jena100-bi-retry-v1' if name.startswith('m003') else 'jena100-bi-reference-v1')/'reference'/(Path(name).stem+'.json')
                expected=json.loads(source.read_text());qualify(1,actual,expected)
                assert actual['results']['bindings']==expected['results']['bindings']
                entry['exact_overlap_sequence_pass']=True
                entry['overlap_reference_sha256']=hashlib.sha256(source.read_bytes()).hexdigest()
        except Exception as exc:entry.update(status='fail',error=type(exc).__name__+': '+str(exc))
        report['queries'][name]=entry
        (out/'matrix.json').write_text(json.dumps(report,indent=2))
    report['complete']=len(report['queries'])==4
    report['all_requested_pass']=report['complete'] and all(v['status']=='pass' for v in report['queries'].values())
    (out/'matrix.json').write_text(json.dumps(report,indent=2))
    return report
