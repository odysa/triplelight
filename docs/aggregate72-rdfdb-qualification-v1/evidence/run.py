import json,hashlib,sys,subprocess,os,time,urllib.request,urllib.parse
from pathlib import Path
root=Path('/private/tmp/rdf-jianlin-analytical25');p=Path(__file__).resolve().parent;prep=root/'aggregate72-preparation-v1';owner='Jianlin task25 aggregate72 rdfdb qualification v1'
sys.path.insert(0,str(prep/'source'));from aggregate_count_policy import compare
f=json.loads((prep/'rules-freeze.json').read_text());assert hashlib.sha256((prep/'rules-freeze.json').read_bytes()).hexdigest()=='5cf92a76e599d1e8a1db1f56a3f5215c3b05fea989665754c02ce3f76bb7f365'
refs=json.loads((root/'aggregate72-reference-v2/reference-binding.json').read_text());binary=root/'exact-decimal-9a2f49b/rdfdb-9a2f49b'
def sha(file):
 h=hashlib.sha256()
 with Path(file).open('rb') as stream:
  while b:=stream.read(8*1024*1024):h.update(b)
 return h.hexdigest()
assert sha(binary)==(binary.parent/'binary-sha256.txt').read_text().strip()
s={'complete':False,'datasets':{},'comparative_timing_claim':False,'source':'9a2f49b652a73466873cacd0a9205098e27d15f6','binary_sha256':sha(binary),'started':time.time()};server=None
def save():(p/'status.json').write_text(json.dumps(s,indent=2))
def query(q):
 req=urllib.request.Request('http://127.0.0.1:31447/sparql',data=q.encode(),headers={'Content-Type':'application/sparql-query','Accept':'application/sparql-results+json'})
 with urllib.request.urlopen(req,timeout=610) as response:return response.read()
try:
 for dataset,identity in f['inputs'].items():
  assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
  d=p/dataset;d.mkdir();entry={'queries':{},'input_before':sha(identity['path'])};s['datasets'][dataset]=entry;save();assert entry['input_before']==identity['sha256']
  cmd=[str(binary),'serve',identity['path'],'31447','--parallel-load=4'];entry['command']=cmd
  with (d/'server.log').open('xb') as log:server=subprocess.Popen(cmd,stdout=log,stderr=subprocess.STDOUT,env={**os.environ,'RAYON_NUM_THREADS':'4','RDFDB_HOST':'127.0.0.1'})
  entry['pid']=server.pid;save()
  try:
   deadline=time.monotonic()+600
   while 'statistics ready after' not in (d/'server.log').read_text(errors='replace'):
    if server.poll() is not None:raise RuntimeError('Server exited during readiness')
    if time.monotonic()>deadline:raise TimeoutError('Stats readiness expired')
    time.sleep(.2)
   (d/'dataset-count.json').write_bytes(query('SELECT (COUNT(*) AS ?n) WHERE {?s ?p ?o}'))
   for name,record in f['queries'].items():
    if record['dataset']!=dataset:continue
    q=prep/'workload'/name;assert sha(q)==record['query_sha256']
    raw=query(q.read_text());out=d/(Path(name).stem+'.json');out.write_bytes(raw)
    ref=root/'aggregate72-reference-v2'/dataset/(Path(name).stem+'.json');assert sha(ref)==refs['references'][name]['sha256']
    result=compare(json.loads(raw),json.loads(ref.read_text()),record['columns']);result['output_sha256']=sha(out);entry['queries'][name]=result;save()
  except Exception as exc:entry['error']=repr(exc)
  finally:
   if server.poll() is None:
    server.terminate()
    try:server.wait(timeout=20)
    except subprocess.TimeoutExpired:server.kill();server.wait()
   entry['server_exit']=server.returncode;server=None
   entry['input_after']=sha(identity['path']);assert entry['input_after']==entry['input_before'];save()
 s['complete']=len(s['datasets'])==2 and all(len(e['queries'])==sum(r['dataset']==n for r in f['queries'].values()) and not e.get('error') for n,e in s['datasets'].items())
finally:
 if server is not None and server.poll() is None:server.terminate();server.wait(timeout=30)
 s['ended']=time.time();save()
