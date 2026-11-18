from pathlib import Path
import subprocess,json,secrets,os,time,urllib.request,urllib.parse
s=Path('/private/tmp/rdf-jianlin-analytical25/matched-vm-10m');store=s/'virtuoso-v3';assert store.is_dir()
assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()=='Jianlin-Su task25 BI 10M staging'
source=Path('<agent-workspace>/rdfdb/bench/tools')
name='rdf-jianlin-bi-virtuoso-10m-v4';secret=(s/'.virtuoso-v3.secret').read_text();env={**os.environ,'DBA_PASSWORD':secret}
command=['docker','run','-d','--rm','--name',name,'--cpus','4','--memory','12g','-p','127.0.0.1:31428:8890','-v',str(store)+':/database','-v',str(source)+':/import:ro','-e','DBA_PASSWORD',
 '-e','VIRT_Parameters_NumberOfBuffers=1360000','-e','VIRT_Parameters_MaxDirtyBuffers=1000000','-e','VIRT_Parameters_ThreadsPerQuery=4','-e','VIRT_Parameters_DirsAllowed=.,/import,/usr/share/proj',
 '-e','VIRT_SPARQL_ResultSetMaxRows=1000000000','-e','VIRT_SPARQL_MaxQueryExecutionTime=120',
 'openlink/virtuoso-opensource-7@sha256:0dbe1ab4fa0cb7bbafc1f6c0c2b0a5d6f22d918dbd17672f2ddb24580aa6756a']
(s/'virtuoso-v4-start-command.json').write_text(json.dumps(command,indent=2));identifier=subprocess.check_output(command,env=env,text=True).strip();(s/'virtuoso-v4-container-id').write_text(identifier)
def sql(text,timeout=15):
 return subprocess.run(['docker','exec','-i',name,'sh','-c','exec isql 1111 dba "$DBA_PASSWORD" VERBOSE=OFF PROMPT=OFF'],input=text.replace(';',';\n')+'\n',text=True,capture_output=True,timeout=timeout)
def safe(text):return text.replace(secret,'<redacted>')
try:
 deadline=time.monotonic()+60
 while True:
  p=sql('select 1;')
  if p.returncode==0 and 'Error' not in p.stdout+p.stderr:break
  if time.monotonic()>deadline:raise TimeoutError('Virtuoso SQL readiness')
  time.sleep(.2)
 (s/'virtuoso-v4-ready.log').write_text(safe(p.stdout+p.stderr))
 text="ld_dir('/import', 'bsbm10.nt', 'http://bench/bsbm10'); rdf_loader_run(); checkpoint; select ll_state,ll_error from DB.DBA.load_list;"
 start=time.monotonic();p=sql(text,timeout=600);(s/'virtuoso-v4-load.log').write_text(safe(p.stdout+p.stderr));(s/'virtuoso-v4-load-exit.json').write_text(json.dumps({'exit_code':p.returncode,'setup_wall_seconds':time.monotonic()-start,'scope':'setup only, not a controlled load comparison'}))
 if p.returncode or '*** Error' in p.stdout+p.stderr:raise RuntimeError('Virtuoso load failed; see retained log')
 query='SELECT (COUNT(*) AS ?count) WHERE { ?s ?p ?o }'
 request=urllib.request.Request('http://127.0.0.1:31428/sparql',data=urllib.parse.urlencode({'query':query,'default-graph-uri':'http://bench/bsbm10','format':'application/sparql-results+json'}).encode(),headers={'Content-Type':'application/x-www-form-urlencoded','Accept':'application/sparql-results+json'})
 with urllib.request.urlopen(request,timeout=120) as response:raw=response.read()
 (s/'virtuoso-v4-graph-count.json').write_bytes(raw);value=json.loads(raw)['results']['bindings'][0]['count'];assert int(value['value'])==10102032
 print('Virtuoso fresh graph count matches 10,102,032')
finally:
 logs=subprocess.run(['docker','logs',name],capture_output=True,text=True);(s/'virtuoso-v4-start.log').write_text(safe(logs.stdout+logs.stderr));p=sql('checkpoint;',timeout=60);(s/'virtuoso-v4-checkpoint.log').write_text(safe(p.stdout+p.stderr));subprocess.run(['docker','stop','--timeout','2',name],capture_output=True,check=True)
