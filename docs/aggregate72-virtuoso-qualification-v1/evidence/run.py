import hashlib,json,subprocess,sys,os,time,signal,urllib.request,urllib.parse,stat
from pathlib import Path
root=Path('/private/tmp/rdf-jianlin-analytical25');p=Path(__file__).resolve().parent;prep=root/'aggregate72-preparation-v1';owner='Jianlin task25 aggregate72 Virtuoso preparation and qualification v1'
sys.path.insert(0,str(prep/'source'));from aggregate_count_policy import compare
f=json.loads((prep/'rules-freeze.json').read_text());refs=json.loads((root/'aggregate72-reference-v2/reference-binding.json').read_text());image='openlink/virtuoso-opensource-7@sha256:0dbe1ab4fa0cb7bbafc1f6c0c2b0a5d6f22d918dbd17672f2ddb24580aa6756a';endpoint='http://127.0.0.1:31449/sparql'
secret_path=root/'matched-vm-10m/.virtuoso-v3.secret';assert stat.S_IMODE(secret_path.stat().st_mode)==0o600;secret=secret_path.read_text().strip();assert secret
def safe(raw):return raw.replace(secret.encode(),b'[REDACTED]')
def sha(file):
 h=hashlib.sha256()
 with Path(file).open('rb') as stream:
  while b:=stream.read(8*1024*1024):h.update(b)
 return h.hexdigest()
def ask(query,graph,timeout=610):
 req=urllib.request.Request(endpoint,data=urllib.parse.urlencode({'query':query,'default-graph-uri':graph,'format':'application/sparql-results+json'}).encode(),headers={'Accept':'application/sparql-results+json'})
 with urllib.request.urlopen(req,timeout=timeout) as response:return response.read()
def cleanup(name,d):
 check=subprocess.run(['docker','inspect',name,'--format','{{json .State}}'],capture_output=True,text=True)
 if check.returncode==0:
  state=json.loads(check.stdout);(d/'container-before-cleanup.json').write_text(check.stdout)
  if state['Running']:subprocess.run(['docker','stop','--time','10',name],check=True,capture_output=True)
  final=subprocess.run(['docker','inspect',name,'--format','{{json .State}}'],check=True,capture_output=True,text=True);(d/'container-after-stop.json').write_text(final.stdout);assert not json.loads(final.stdout)['Running']
  logs=subprocess.run(['docker','logs',name],capture_output=True);(d/'server.log').write_bytes(safe(logs.stdout+logs.stderr));subprocess.run(['docker','rm',name],check=True,capture_output=True)
def interrupted(signum,frame):raise KeyboardInterrupt('owned Virtuoso attempt interrupted')
signal.signal(signal.SIGTERM,interrupted)
s={'complete':False,'datasets':{},'comparative_timing_claim':False,'comparative_load_claim':False,'image':image,'image_architecture':'arm64','started':time.time()};active=None;d=None
def save():(p/'status.json').write_text(json.dumps(s,indent=2))
try:
 assert sha(prep/'rules-freeze.json')=='5cf92a76e599d1e8a1db1f56a3f5215c3b05fea989665754c02ce3f76bb7f365'
 for dataset,identity in f['inputs'].items():
  assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
  d=p/dataset;d.mkdir();(d/'database').mkdir();graph='http://bench/aggregate72/'+dataset;name='rdf-jianlin-agg72-virt-'+dataset+'-v1';active=name
  entry={'queries':{},'input_before':sha(identity['path']),'graph':graph};s['datasets'][dataset]=entry;save();assert entry['input_before']==identity['sha256']
  cmd=['docker','run','--detach','--name',name,'--cpus=4','--memory=12g','--memory-swap=12g','-p','127.0.0.1:31449:8890','--mount','type=bind,source='+str(d/'database')+',target=/database','--mount','type=bind,source='+identity['path']+',target=/import/data.nt,readonly','--env','DBA_PASSWORD','--env','VIRT_Parameters_NumberOfBuffers=1360000','--env','VIRT_Parameters_MaxDirtyBuffers=1000000','--env','VIRT_Parameters_ThreadsPerQuery=4','--env','VIRT_Parameters_DirsAllowed=.,/import,/usr/share/proj','--env','VIRT_SPARQL_ResultSetMaxRows=1000000000','--env','VIRT_SPARQL_MaxQueryExecutionTime=600',image]
  entry['command']=cmd;save();run=subprocess.run(cmd,env={**os.environ,'DBA_PASSWORD':secret},capture_output=True);(d/'start.stdout').write_bytes(safe(run.stdout));(d/'start.stderr').write_bytes(safe(run.stderr));assert run.returncode==0;entry['container_id']=run.stdout.decode().strip();save()
  try:
   deadline=time.monotonic()+180
   while True:
    try:
     if json.loads(ask('ASK {}',graph,10)).get('boolean') is True:break
    except Exception:pass
    if time.monotonic()>deadline:raise TimeoutError('Virtuoso readiness expired')
    time.sleep(.3)
   sql="log_enable(2,1);\nDB.DBA.ld_dir('/import', 'data.nt', '"+graph+"');\nDB.DBA.rdf_loader_run();\ncheckpoint;\nselect ll_file,ll_state,ll_error from DB.DBA.load_list;\n"
   (d/'load.sql').write_text(sql)
   # Only the container-local shell expands the existing private credential. Host argv/logs contain no value.
   load_cmd=['docker','exec','-i',name,'bash','-c','exec "$VIRTUOSO_HOME/bin/isql" 1111 dba "$(cat /settings/dba_password)" VERBOSE=OFF']
   entry['load_command']=load_cmd;save();proc=subprocess.run(load_cmd,input=sql.encode(),capture_output=True,timeout=1800);(d/'load.stdout').write_bytes(safe(proc.stdout));(d/'load.stderr').write_bytes(safe(proc.stderr));entry['loader_exit']=proc.returncode;save();assert proc.returncode==0 and b'*** Error' not in proc.stdout+proc.stderr
   raw=ask('SELECT (COUNT(*) AS ?n) WHERE {?s ?p ?o}',graph);(d/'dataset-count.json').write_bytes(raw);actual=int(json.loads(raw)['results']['bindings'][0]['n']['value']);ref=json.loads((root/'aggregate72-reference-v2'/dataset/'dataset-count.json').read_text());assert actual==int(ref['results']['bindings'][0]['n']['value']);entry['graph_count']=actual;save()
   for qname,record in f['queries'].items():
    if record['dataset']!=dataset:continue
    q=prep/'workload'/qname;assert sha(q)==record['query_sha256'];raw=ask(q.read_text(),graph);out=d/(Path(qname).stem+'.json');out.write_bytes(raw);ref=root/'aggregate72-reference-v2'/dataset/(Path(qname).stem+'.json');assert sha(ref)==refs['references'][qname]['sha256']
    result=compare(json.loads(raw),json.loads(ref.read_text()),record['columns']);result['output_sha256']=sha(out);entry['queries'][qname]=result;save()
  except Exception as exc:entry['error']=repr(exc).replace(secret,'[REDACTED]')
  finally:
   cleanup(active,d);active=None;entry['input_after']=sha(identity['path']);entry['input_unchanged']=entry['input_before']==entry['input_after'];save()
 s['complete']=len(s['datasets'])==2 and all(len(e['queries'])==sum(r['dataset']==n for r in f['queries'].values()) and not e.get('error') and e['input_unchanged'] for n,e in s['datasets'].items())
finally:
 if active is not None:cleanup(active,d)
 s['ended']=time.time();save()
