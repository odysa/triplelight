#!/usr/bin/env python3
import hashlib,json,tarfile,tempfile
from pathlib import Path
from collections import Counter
ROOT=Path(__file__).resolve().parent
MODES=['memory','disk-load','disk-reopen']
LABELS=['group','plain','distinct','count','distinct_count','empty','unbound','ask_false']
def key(t):
    if t is None:return None
    if t['type']=='literal':
        lang=t.get('xml:lang','').lower()
        return ('literal',t['value'],t.get('datatype') or ('http://www.w3.org/1999/02/22-rdf-syntax-ns#langString' if lang else 'http://www.w3.org/2001/XMLSchema#string'),lang)
    return (t['type'],t['value'])
def bag(a):return Counter(tuple(key(r.get(v)) for v in a['head']['vars']) for r in a['results']['bindings'])
def main():
    manifest=json.loads((ROOT/'PACKET_FILES.json').read_text())
    with tempfile.TemporaryDirectory(prefix='ox39-replay-') as d:
        root=Path(d)
        with tarfile.open(ROOT/'packet.tar.gz') as tar:
            assert all(m.isfile() for m in tar.getmembers())
            names=[m.name for m in tar.getmembers()];assert len(names)==len(set(names))
            tar.extractall(root,filter='data')
        assert {str(p.relative_to(root)) for p in root.rglob('*') if p.is_file()}==set(manifest)
        for n,h in manifest.items():assert hashlib.sha256((root/n).read_bytes()).hexdigest()==h,n
        source=root/'source';freeze=json.loads((source/'FREEZE.json').read_text())
        assert hashlib.sha256((source/'FREEZE.json').read_bytes()).hexdigest()=='3021ddcda7744461742d427ae4e11b62bd550a16d5325606b09b5d7faf7d913e'
        for n,h in freeze['files'].items():assert hashlib.sha256((source/n).read_bytes()).hexdigest()==h
        ref=json.loads((source/'consumer-fixture-jena.json').read_text())
        objects={'head':{'vars':['o']},'results':{'bindings':[{'o':r['o']} for r in ref['results']['bindings']]}}
        count={'head':{'vars':['n']},'results':{'bindings':[{'n':{'type':'literal','value':'9','datatype':'http://www.w3.org/2001/XMLSchema#integer'}}]}}
        n=0
        for mode in MODES:
            p=root/'attempt-1'/mode;summary=json.loads((p/'summary.json').read_text())
            assert summary['mode']==mode and summary['stored_quads']==9 and summary['tiny_answers_pass'] is True and summary['full_workload_qualified'] is False
            for label in LABELS:
                r=json.loads((p/(label+'.json')).read_text());assert r['pass'] is True and all(v is True for v in r['checks'].values()) and 'error' not in r
                a=r['answer']
                if label=='ask_false':assert a=={'boolean':False}
                elif label=='empty':assert a=={'head':{'vars':['o']},'results':{'bindings':[]}}
                elif label=='unbound':
                    assert a['head']['vars']==['o','missing'] and all('missing' not in b for b in a['results']['bindings'])
                    assert bag({'head':{'vars':['o']},'results':a['results']})==bag(objects)
                else:
                    expected=ref if label=='group' else count if label in ['count','distinct_count'] else objects
                    assert a['head']['vars']==expected['head']['vars'] and bag(a)==bag(expected)
                assert r['explain'].startswith('-- oxilite: fully compiled to SQL ')
                n+=1
        sup=json.loads((root/'attempt-1/supervisor.json').read_text())
        assert sup['admitted'] is True and sup['full_workload_qualified'] is False and sup['identities_unchanged'] is True
        assert [r['mode'] for r in sup['children']]==MODES and all(r['exit_code']==0 and r['timeout'] is False for r in sup['children'])
        post=json.loads((root/'POSTFLIGHT.json').read_text());assert post['exit_code']==0 and post['installed_runtime_unchanged'] is True
        execution=root/'EXECUTION.json';assert hashlib.sha256(execution.read_bytes()).hexdigest()=='d29210b5d9339d9c663a2a71adf2c90ab6ffaf5544e990d6a77e55e137dd4d9c'
        e=json.loads(execution.read_text());assert hashlib.sha256((root/'RUNTIME.json').read_bytes()).hexdigest()==e['runtime_sha256']
        print(json.dumps({'packet_files_verified':len(manifest),'observations_replayed':n,'tiny_answer_admitted':True,'full_workload_qualified':False,'engine_executed':False}))
if __name__=='__main__':main()
