from pathlib import Path
import subprocess,json,urllib.request,urllib.parse,time,socket,sys,hashlib,os
root=Path(__file__).resolve().parent;s=root/'matched-vm-10m';sys.path.insert(0,str(s/'bench-source'))
from http_measure import verify
from envelope_qualify import FrozenInputs
from count_equivalence import normalize,load_freeze
owner='Jianlin-Su task25 BI 10M staging';assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
w=s/'bench-source/workloads/bsbm10';manifest=json.loads((w/'manifest.json').read_text());mode=sys.argv[1];out=s/('probe-'+mode+'-retry1');out.mkdir(exist_ok=False)
freeze=Path('<agent-workspace>/rdfdb/docs/analytical-benchmark/count-equivalence-v1/freeze.json')
expected='ffa49a6ce10c19c02d6185a3431383157c590e1e1beb9c9d37b7b83e03bf5790';assert load_freeze(freeze,expected)==expected
numeric=FrozenInputs(root/'numeric-diagnostics-10m','a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244',manifest['source_data_sha256'],hashlib.sha256((w/'manifest.json').read_bytes()).hexdigest())
port={'rdfdb':31429,'qlever':31430,'virtuoso':31428}[mode];name='rdf-jianlin-bi-'+mode+'-probe'
with socket.socket() as sock:sock.bind(('127.0.0.1',port))
command=['docker','run','-d','--rm','--name',name,'--cpus','4','--memory','12g','-p',f'127.0.0.1:{port}:{port}','-v',str(s)+':/work']
if mode=='rdfdb':
 command+=['-e','RAYON_NUM_THREADS=4','-e','RDFDB_HOST=0.0.0.0','--entrypoint','/work/target-linux/release/rdfdb','rust@sha256:0b0ea86881537b3f645aace7e8e39556289f8787db057317c8d4978548213b9e','serve','/work/rdfdb-disk',str(port),'--disk','--cache-mb=10240']
 endpoint=f'http://127.0.0.1:{port}/sparql'
elif mode=='qlever':
 command+=['-w','/work/qlever','--entrypoint','/qlever/qlever-server','adfreiburg/qlever@sha256:d7ecda47b4d0132c190b4a73279761fd1c614f24cba4e894ca1ba8117cef6727','-i','bsbm10','-p',str(port),'-m','11GB','-c','1MB','-e','1MB','-E','1MB','-s','120s','--set-runtime-parameter','max-num-threads-per-query=4','--no-metrics-log','--no-resource-usage-log']
 endpoint=f'http://127.0.0.1:{port}/'
else:
 command=json.loads((s/'virtuoso-v5-start-command.json').read_text())
 command[command.index('--name')+1]=name
 endpoint=f'http://127.0.0.1:{port}/sparql'
 environment={**os.environ,'DBA_PASSWORD':(s/'.virtuoso-v3.secret').read_text()} if mode=='virtuoso' else None
(out/'command.json').write_text(json.dumps(command,indent=2));identifier=subprocess.check_output(command,text=True,env=environment).strip();(out/'container-id').write_text(identifier)
def ask(fields,name):
 if mode=='virtuoso' and 'query' in fields:fields={**fields,'default-graph-uri':'http://bench/bsbm10','format':'application/sparql-results+json'}
 if mode=='virtuoso' and 'query' in fields:fields={**fields,'default-graph-uri':'http://bench/bsbm10','format':'application/sparql-results+json'}
 request=urllib.request.Request(endpoint,data=urllib.parse.urlencode(fields).encode(),headers={'Content-Type':'application/x-www-form-urlencoded','Accept':'application/sparql-results+json'})
 with urllib.request.urlopen(request,timeout=120) as response:raw=response.read()
 (out/(name+'.json')).write_bytes(raw);return json.loads(raw)
try:
 deadline=time.monotonic()+120
 while True:
  logs=subprocess.run(['docker','logs',name],capture_output=True,text=True)
  if mode=='rdfdb':ready='statistics ready after' in logs.stderr+logs.stdout
  else:
   try:
    with socket.create_connection(('127.0.0.1',port),timeout=.2):ready=True
   except OSError:ready=False
  if ready:break
  if time.monotonic()>deadline:raise TimeoutError('probe readiness')
  time.sleep(.2)
 if mode=='qlever':
  settings=ask({'cmd':'get-settings'},'settings');assert settings['memory-for-queries']=='11 GB' and settings['max-num-threads-per-query']=='4'
 deadline=time.monotonic()+120;attempts=[]
 while True:
  try:
   count=ask({'query':'SELECT (COUNT(*) AS ?count) WHERE { ?s ?p ?o }'},'graph-count')
   assert int(count['results']['bindings'][0]['count']['value'])==10102032
   break
  except (OSError, __import__('http.client',fromlist=['RemoteDisconnected']).RemoteDisconnected) as exc:
   attempts.append(type(exc).__name__+': '+str(exc))
   if time.monotonic()>deadline:raise
   time.sleep(.25)
 (out/'readiness-transport-attempts.json').write_text(json.dumps(attempts,indent=2))
 report={'mode':mode,'graph_count':10102032,'queries':{},'timing_score':None}
 for item in manifest['queries'][:2]:
  file=item['file'];stem=Path(file).stem
  if mode=='qlever':
   counters=ask({'cmd':'clear-cache'},stem+'-cache-control');assert len(counters)==5 and all(v==0 for v in counters.values())
  actual=ask({'query':(w/'queries'/file).read_text()},stem)
  reference=json.loads((root/'bsbm10-holdout-v1/jena'/(stem+'.json')).read_text());entry={}
  for policy,a,b in [('strict',actual,reference),('count_equivalence',normalize(item['template'],actual),normalize(item['template'],reference))]:
   try:entry[policy]={'status':'pass',**verify(item['template'],a,b,None,numeric,item)}
   except Exception as exc:entry[policy]={'status':'fail','error':type(exc).__name__+': '+str(exc)}
  report['queries'][file]=entry;(out/'result.json').write_text(json.dumps(report,indent=2));print(mode,file,{k:v['status'] for k,v in entry.items()},flush=True)
finally:
 logs=subprocess.run(['docker','logs',name],capture_output=True,text=True);(out/'server.log').write_text(logs.stdout+logs.stderr)
 resource=subprocess.run(['docker','exec',name,'sh','-c','cat /proc/1/status; cat /sys/fs/cgroup/memory.peak'],capture_output=True,text=True);(out/'resource-end.txt').write_text(resource.stdout+resource.stderr)
 subprocess.run(['docker','stop','--timeout','2',name],capture_output=True,check=True)
