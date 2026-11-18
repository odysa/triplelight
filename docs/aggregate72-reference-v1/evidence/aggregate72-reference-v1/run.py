import json,os,time,signal,subprocess,hashlib,urllib.request,urllib.parse,traceback,shutil
from pathlib import Path
ROOT=Path('/private/tmp/rdf-jianlin-analytical25')
STAGE=Path(__file__).resolve().parent
PREP=ROOT/'aggregate72-preparation-v1'
LOCK=Path('/tmp/rdf-bench.lock/owner')
OWNER='Jianlin task25 aggregate72 Jena reference v1'
JENA=Path('<agent-workspace>/rdfdb/bench/tools/apache-jena-fuseki-6.2.0')
PORT=31446
state={'complete':False,'comparative_timing_claim':False,'datasets':{},'owner':OWNER,'started':time.time()}
def save(): (STAGE/'status.json').write_text(json.dumps(state,indent=2))
def sha(p):
 h=hashlib.sha256()
 with Path(p).open('rb') as f:
  for b in iter(lambda:f.read(2**20),b''):h.update(b)
 return h.hexdigest()
def guard():
 if LOCK.read_text().strip()!=OWNER:raise RuntimeError('Lost own benchmark lock')
 if shutil.disk_usage(ROOT).free<=48*1024**3:raise RuntimeError('Conservative free-space guard')
def query(q,timeout=610):
 request=urllib.request.Request(f'http://127.0.0.1:{PORT}/ds/query',data=urllib.parse.urlencode({'query':q,'timeout':'600000'}).encode(),headers={'Accept':'application/sparql-results+json'})
 with urllib.request.urlopen(request,timeout=timeout) as r:return r.read()
process=None
try:
 guard();freeze=json.loads((PREP/'rules-freeze.json').read_text())
 if sha(PREP/'rules-freeze.json')!='5cf92a76e599d1e8a1db1f56a3f5215c3b05fea989665754c02ce3f76bb7f365':raise RuntimeError('Rules changed')
 jar=JENA/'fuseki-server.jar'
 state['jar']={'path':str(jar),'sha256':sha(jar)};save()
 for dataset,identity in freeze['inputs'].items():
  guard();entry={'queries':{},'input_before':sha(identity['path'])};state['datasets'][dataset]=entry;save()
  if entry['input_before']!=identity['sha256']:raise RuntimeError('Input mismatch')
  d=STAGE/dataset;d.mkdir()
  cmd=['java','-Xmx8g','-XX:ActiveProcessorCount=4','-jar',str(jar),'--localhost','--port='+str(PORT),'--file='+identity['path'],'/ds']
  entry['command']=cmd;env=dict(os.environ,FUSEKI_BASE=str(d/'run'),FUSEKI_HOME=str(JENA))
  log=(d/'server.log').open('wb');process=subprocess.Popen(cmd,cwd=d,env=env,stdout=log,stderr=subprocess.STDOUT)
  entry['pid']=process.pid;save()
  try:
   ready=False
   for _ in range(600):
    guard()
    if process.poll() is not None:raise RuntimeError('Jena exited during startup: '+str(process.returncode))
    try:
     body=query('SELECT (COUNT(*) AS ?n) WHERE {?s ?p ?o}',timeout=2)
     (d/'dataset-count.json').write_bytes(body);ready=True;break
    except (OSError,TimeoutError):time.sleep(1)
   if not ready:raise RuntimeError('Jena startup readiness expired')
   for name,record in freeze['queries'].items():
    if record['dataset']!=dataset:continue
    guard();qfile=PREP/'workload'/name
    if sha(qfile)!=record['query_sha256']:raise RuntimeError('Query mismatch')
    result={'query_sha256':record['query_sha256']};entry['queries'][name]=result;save()
    try:
     body=query(qfile.read_text());obj=json.loads(body)
     if 'results' not in obj:raise RuntimeError('Not a SELECT result')
     out=d/(Path(name).stem+'.json');out.write_bytes(body)
     result.update(response_sha256=sha(out),rows=len(obj['results']['bindings']),status='ok')
    except Exception as exc:
     result.update(status='failed',error=repr(exc));save();raise
    save()
  except Exception as exc:entry['error']=repr(exc)
  finally:
   if process.poll() is None:
    process.terminate()
    try:process.wait(timeout=20)
    except subprocess.TimeoutExpired:process.kill();process.wait()
   entry['server_exit']=process.returncode;log.close();process=None
   entry['input_after']=sha(identity['path']);entry['input_unchanged']=entry['input_after']==entry['input_before'];save()
 state['complete']=all(len(e['queries'])==sum(r['dataset']==n for r in freeze['queries'].values()) and all(q['status']=='ok' for q in e['queries'].values()) and e['input_unchanged'] for n,e in state['datasets'].items()) and len(state['datasets'])==2
except BaseException as exc:state['error']=repr(exc);traceback.print_exc()
finally:
 if process is not None and process.poll() is None:process.terminate();process.wait(timeout=30)
 state['ended']=time.time();save()
