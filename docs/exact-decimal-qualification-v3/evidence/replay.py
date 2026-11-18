import json,subprocess,hashlib,sys,collections
from pathlib import Path
from fractions import Fraction
p=Path(__file__).resolve().parent;sys.path.insert(0,str(p/'bench-source'))
from qualify import exact_term
refs=json.loads((p/'reference-identity.json').read_text())
for n,h in refs['files'].items():assert hashlib.sha256((p/'controls'/n).read_bytes()).hexdigest()==h
unbound=set('coefficient-overflow scale-29 integer-overflow decimal-overflow beyond-decimal-coefficient sub-scale-29 max-add-fraction fraction-add-max product-coefficient-rounding terminating-underflow decimal-zero'.split())
def bag(x):return collections.Counter(tuple((n,exact_term(t)) for n,t in sorted(row.items())) for row in x['results']['bindings'])
r={};cmds={}
for name,h in json.loads((p/'control-manifest.json').read_text()).items():
 q=p/'controls'/(name+'.rq');assert hashlib.sha256(q.read_bytes()).hexdigest()==h
 cmd=[str(p/'rdfdb-9a2f49b'),'query',str(p/'fixture.nt'),q.read_text()]
 process=subprocess.run(cmd,capture_output=True,timeout=45,env={**__import__('os').environ,'RAYON_NUM_THREADS':'1'})
 (p/'controls'/(name+'-candidate.json')).write_bytes(process.stdout);(p/'controls'/(name+'-candidate.stderr')).write_bytes(process.stderr)
 cmds[name]={'command':cmd,'exit':process.returncode};assert process.returncode==0,(name,process.stderr)
 actual=json.loads(process.stdout);entry={'contract_pass':False,'jena_exact_pass':None}
 if name!='decimal-zero':
  reference=json.loads((p/'controls'/(name+'-jena.json')).read_text());entry['jena_exact_pass']=bag(actual)==bag(reference)
 if name in unbound:
  entry['classification']='declared exact-or-error boundary' if name!='decimal-zero' else 'contract error; Jena oracle exception unscored'
  entry['contract_pass']=actual['head']['vars']==['v'] and actual['results']['bindings']==[{}]
 elif name=='repeating-division':
  value=actual['results']['bindings'][0]['v'];entry['classification']='nonterminating decimal precision'
  entry['contract_pass']=value['datatype'].endswith('#decimal') and abs(Fraction(value['value'])-Fraction(1,3))<=Fraction(1,2*10**28)
 elif name=='mixed-double':
  entry['classification']='double lexical spelling'
  value=actual['results']['bindings'][0]['v'];expected=reference['results']['bindings'][0]['v']
  entry['contract_pass']=value['datatype']==expected['datatype'] and float(value['value'])==float(expected['value'])
 else:entry['classification']='exact Jena term bag';entry['contract_pass']=entry['jena_exact_pass'] is True
 r[name]=entry
 (p/'replay-results.json').write_text(json.dumps(r,indent=2))
 (p/'replay-commands.json').write_text(json.dumps(cmds,indent=2))
print('contract',sum(e['contract_pass'] for e in r.values()),'/',len(r),'exact',sum(e['jena_exact_pass'] is True for e in r.values()))
assert all(e['contract_pass'] for e in r.values()),r
