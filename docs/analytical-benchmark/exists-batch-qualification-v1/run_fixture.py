from pathlib import Path
from collections import Counter
import os,sys,json,hashlib,subprocess,time,signal,urllib.request,urllib.parse
root=Path(__file__).parent;f=root/'correlation-fixture';parent=root.parent
owner='Jianlin-Su task25 exists-batch6e14248 qualification';assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
repo=Path('<agent-workspace>/rdfdb');sys.path.insert(0,str(repo/'bench/analytical'));from qualify import exact_term
queries=sorted(f.glob('*.rq'));expected=json.loads((f/'expected-counts.json').read_text());processes={}
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
inputs={p.name:sha(p) for p in [f/'data.nt',f/'expected-counts.json',*queries]}
(f/'input-identities-before.json').write_text(json.dumps(inputs,indent=2))
for mode,binary,port in [('main',parent/'probe-set-1cd8881/rdfdb',31436),('candidate',root/'rdfdb',31437)]:
 out=f/mode;out.mkdir();cmd=[str(binary),'serve',str(f/'data.nt'),str(port),'--no-stats','--parallel-load=1'];(out/'command.json').write_text(json.dumps({'command':cmd,'binary_sha256':sha(binary),'env':{'RAYON_NUM_THREADS':'1'}},indent=2))
 with (out/'server.log').open('wb') as log:
  server=subprocess.Popen(cmd,env={**os.environ,'RAYON_NUM_THREADS':'1'},stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
  def query(text):
   req=urllib.request.Request(f'http://127.0.0.1:{port}/sparql',data=urllib.parse.urlencode({'query':text}).encode(),headers={'Accept':'application/sparql-results+json'})
   with urllib.request.urlopen(req,timeout=30) as response:return response.read()
  try:
   deadline=time.monotonic()+30
   while True:
    if server.poll() is not None:raise RuntimeError('owned fixture server exited')
    try:query('ASK {}');break
    except OSError:
     if time.monotonic()>deadline:raise
     time.sleep(.1)
   for q in queries:
    raw=query(q.read_text());json.loads(raw);(out/(q.stem+'.json')).write_bytes(raw)
  finally:
   if server.poll() is None:os.killpg(server.pid,signal.SIGTERM);server.wait(timeout=10)
   processes[mode]={'pid':server.pid,'exit':server.returncode}
java=Path('<agent-workspace>/rdfdb/bench/tools/apache-jena-6.2.0');arq=java/'bin/arq';out=f/'jena';out.mkdir();commands=[]
for q in queries:
 cmd=[str(arq),'--data='+str(f/'data.nt'),'--query='+str(q),'--results=JSON'];p=subprocess.run(cmd,env={**os.environ,'JAVA_TOOL_OPTIONS':'-Xmx512m -XX:ActiveProcessorCount=1'},capture_output=True,timeout=45)
 (out/(q.stem+'.json')).write_bytes(p.stdout);(out/(q.stem+'.stderr')).write_bytes(p.stderr);assert p.returncode==0;json.loads(p.stdout);commands.append({'query':q.name,'command':cmd,'exit':p.returncode})
(out/'commands.json').write_text(json.dumps(commands,indent=2));(out/'identity.json').write_text(json.dumps({'arq_script_sha256':sha(arq),'jars':{x.name:sha(x) for pat in ['jena-arq*.jar','jena-core*.jar'] for x in (java/'lib').glob(pat)},'JAVA_TOOL_OPTIONS':'-Xmx512m -XX:ActiveProcessorCount=1'},indent=2))
result={}
for q in queries:
 ref=json.loads((out/(q.stem+'.json')).read_text());variables=ref['head']['vars']
 def rows(d):return [tuple(exact_term(row.get(v)) for v in variables) for row in d['results']['bindings']]
 refrows=rows(ref);entry={'expected_rows':expected[q.stem],'jena_rows':len(refrows)};assert len(refrows)==expected[q.stem]
 for mode in ['main','candidate']:
  actual=json.loads((f/mode/(q.stem+'.json')).read_text());a=rows(actual);entry[mode]={'rows':len(a),'exact_Jena_bag':Counter(a)==Counter(refrows),'exact_Jena_sequence':a==refrows};assert set(actual['head']['vars'])==set(variables) and Counter(a)==Counter(refrows) and a==refrows
 result[q.stem]=entry
assert all(sha(f/n)==h for n,h in inputs.items())
(f/'summary.json').write_text(json.dumps({'all12_pass':True,'cases':result,'owned_servers':processes,'input_hashes_unchanged':True,'scope':'Serial main/candidate/Jena correctness fixtures; no timing comparison'},indent=2)+'\n')
print('All12 fixtures match main and independent Jena bags/sequences; servers exited')
