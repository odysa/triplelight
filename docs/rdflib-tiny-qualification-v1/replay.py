#!/usr/bin/env python3
import hashlib,json,tarfile,tempfile
from pathlib import Path
from collections import Counter
ROOT=Path(__file__).resolve().parent
def key(t):
    if t is None:return None
    if t['type']=='literal':
        lang=t.get('xml:lang','').lower();dt=t.get('datatype') or ('http://www.w3.org/1999/02/22-rdf-syntax-ns#langString' if lang else 'http://www.w3.org/2001/XMLSchema#string')
        return ('literal',t['value'],dt,lang)
    return (t['type'],t['value'])
def bag(a):return Counter(tuple(key(r.get(v)) for v in a['head']['vars']) for r in a['results']['bindings'])
def sha(p):return hashlib.sha256(Path(p).read_bytes()).hexdigest()
def main():
    manifest=json.loads((ROOT/'PACKET_FILES.json').read_text())
    with tempfile.TemporaryDirectory(prefix='rdflib39-replay-') as d:
        r=Path(d)
        with tarfile.open(ROOT/'packet.tar.gz') as t:
            members=t.getmembers();assert all(m.isfile() for m in members);assert len({m.name for m in members})==len(members);t.extractall(r,filter='data')
        assert {str(p.relative_to(r)) for p in r.rglob('*') if p.is_file()}==set(manifest)
        for n,h in manifest.items():assert sha(r/n)==h,n
        source=r/'source';assert sha(source/'FREEZE.json')=='5cd10e3658d00092661e43d57497a677e286f53a802ae925cde9b9e8f9307048'
        for n,h in json.loads((source/'FREEZE.json').read_text())['files'].items():assert sha(source/n)==h
        ref=json.loads((source/'consumer-fixture-jena.json').read_text());objects={'head':{'vars':['o']},'results':{'bindings':[{'o':b['o']} for b in ref['results']['bindings']]}}
        computed={}
        for name,normal,lexical in [('default',True,'1'),('preserve-lexical',False,'01')]:
            p=json.loads((r/'attempt-1/child'/(name+'.json')).read_text());assert p['profile']==name and p['normalize_literals'] is normal and p['stored_triples']==9
            a=p['answers'];assert set(a)=={'group','empty','unbound','ask_false'}
            checks={'group_projection':a['group']['head']['vars']==ref['head']['vars'],
                'group_strict_term_bag':bag(a['group'])==bag(ref),
                'empty_projection':a['empty']=={'head':{'vars':['o']},'results':{'bindings':[]}},
                'unbound_projection':a['unbound']['head']['vars']==['o','missing'],
                'unbound_strict_object_bag':bag({'head':{'vars':['o']},'results':a['unbound']['results']})==bag(objects),
                'unbound_cells':all('missing' not in b for b in a['unbound']['results']['bindings']),
                'false_ask':a['ask_false']=={'boolean':False}}
            assert checks==p['checks'] and p['tiny_controls_pass'] is False and not all(checks.values())
            assert a['empty']['head']['vars']==['o'] and len(a['empty']['results']['bindings'])==9
            diag=p['normalization_diagnostic'];assert diag['requested'] is normal and diag['constructor_lexical']==lexical and diag['parsed_int_lexicals']==[lexical]
            computed[name]=checks
        sup=json.loads((r/'attempt-1/supervisor.json').read_text());assert sup['exit_code']==0 and sup['timeout'] is False and sup['source_inputs_unchanged'] is True and sup['profiles_completed'] is True
        assert sup['profile_verdicts']=={'default':False,'preserve-lexical':False} and sup['full_workload_qualified'] is False
        assert sha(r/'EXECUTION.json')=='f574a1fb8f2c2f2b9a3bc92d24cb7876adeda91136b75d601e042b81d076ecdf'
        e=json.loads((r/'EXECUTION.json').read_text());assert sha(r/'RUNTIME.json')==e['runtime_sha256']
        post=json.loads((r/'POSTFLIGHT.json').read_text());assert post['exit_code']==0 and post['installed_runtime_unchanged'] is True and post['child_exited'] is True
        print(json.dumps({'files_verified':len(manifest),'answers_replayed':8,'checks_recomputed':14,'profile_verdicts':sup['profile_verdicts'],'full_workload_qualified':False,'engine_executed':False}))
if __name__=='__main__':main()
