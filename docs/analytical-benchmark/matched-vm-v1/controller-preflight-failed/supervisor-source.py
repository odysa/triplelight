"""Pinned serial two-pass HTTP cohort. Qualification source is independently frozen."""
from pathlib import Path
import subprocess,json,hashlib,os,time,socket,urllib.request,urllib.parse,threading,shutil
root=Path(__file__).resolve().parent;s=root/'matched-vm-10m';out=s/'round1';out.mkdir(exist_ok=False)
owner='Jianlin-Su task25 BI matched-VM round1'
assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
python='<agent-workspace>/scratch/analytical25-venv/bin/python'
helper=s/'bench-source/http_measure.py';workload=s/'bench-source/workloads/bsbm10';manifest=json.loads((workload/'manifest.json').read_text())
policy=Path('<agent-workspace>/rdfdb/docs/analytical-benchmark/count-equivalence-v1/freeze.json')
policy_sha='ffa49a6ce10c19c02d6185a3431383157c590e1e1beb9c9d37b7b83e03bf5790'
assert hashlib.sha256(policy.read_bytes()).hexdigest()==policy_sha
shutil.copy2(Path(__file__),out/'supervisor-source.py');shutil.copy2(policy,out/'count-policy-freeze.json')
order=['rdfdb','duckdb','virtuoso','qlever','qlever-hash','qlever-bypass'];status={}
(out/'schedule.json').write_text(json.dumps({'passes':[order,list(reversed(order))],'per_phase_queries':60,'query_warmup_mix':0,'scored_mixes':[1,2,3],'quality':'strict and count-equivalence separately; failed cohorts have no total','request_timeout_seconds':120,'phase_timeout_seconds':1200,'mode':'HTTP full body consumption; no complete four-engine ranking unless all required outputs qualify'},indent=2))
def ask(endpoint,fields):
 req=urllib.request.Request(endpoint,data=urllib.parse.urlencode(fields).encode(),headers={'Content-Type':'application/x-www-form-urlencoded','Accept':'application/sparql-results+json'})
 with urllib.request.urlopen(req,timeout=120) as response:return response.read()
def message(text):
 subprocess.run(['rtk','proxy','raft','task','reply','(internal discussion)'],input=text,text=True,capture_output=True)
def request_fields(mode,query):
 fields={'query':query}
 if mode=='virtuoso':fields.update({'default-graph-uri':'http://bench/bsbm10','format':'application/sparql-results+json'})
 return fields
for passno,systems in enumerate([order,list(reversed(order))],1):
 for mode in systems:
  assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
  owned_paths=list(Path('/private/tmp').glob('rdf-jianlin-*'))
  usage=subprocess.check_output(['du','-sk',*[str(p) for p in owned_paths]],text=True)
  if sum(int(line.split()[0])*1024 for line in usage.splitlines())>13*1024**3:raise RuntimeError('Owned scratch baseline exceeds 13 GiB; obtain more allocation before another spill-capable phase')
  if shutil.disk_usage(s).free<40*1024**3:raise RuntimeError('Shared free-space floor reached before next phase')
  phase=out/(f'pass{passno}-'+mode);phase.mkdir();name=f'rdf-jianlin-bi-p{passno}-{mode}'
  kind='qlever' if mode.startswith('qlever') else mode;env=None
  if kind=='duckdb':
   command=json.loads((s/'duck-smoke-command.json').read_text());port=31427;endpoint=f'http://127.0.0.1:{port}/sparql'
   command[command.index('--lock-owner')+1]=owner;command[command.index('--spill')+1]='/work/'+str(phase.relative_to(s))+'/spill'
   command[command.index('--max-spill')+1]='6GB'
  elif kind=='virtuoso':
   command=json.loads((s/'virtuoso-v5-start-command.json').read_text());port=31428;endpoint=f'http://127.0.0.1:{port}/sparql'
   env={**os.environ,'DBA_PASSWORD':(s/'.virtuoso-v3.secret').read_text()}
  else:
   command=json.loads((s/('probe-'+kind)/'command.json').read_text());port=31429 if kind=='rdfdb' else 31430
   endpoint=f'http://127.0.0.1:{port}/'+('sparql' if kind=='rdfdb' else '')
   if mode=='qlever-hash':command+=['--set-runtime-parameter','group-by-hash-map-enabled=true']
   if mode=='qlever-bypass':command+=['--set-runtime-parameter','disable-caching=true']
  command[command.index('--name')+1]=name
  if '--rm' in command:command.remove('--rm')
  temporary=phase/'tmp';temporary.mkdir();command[command.index('--entrypoint'):command.index('--entrypoint')]=['-e','TMPDIR=/work/'+str(temporary.relative_to(s))]
  with socket.socket() as sock:sock.bind(('127.0.0.1',port))
  (phase/'server-command.json').write_text(json.dumps(command,indent=2))
  identifier=subprocess.check_output(command,env=env,text=True).strip();(phase/'container-id').write_text(identifier)
  stop=threading.Event();spill_stats={'peak_sampled_named_temp_bytes':0,'sample_seconds':.2,'errors':[]}
  def observe():
   roots=[temporary,phase/'spill']
   while not stop.is_set():
    try:
     total=sum(p.stat().st_size for directory in roots if directory.exists() for p in directory.rglob('*') if p.is_file());spill_stats['peak_sampled_named_temp_bytes']=max(spill_stats['peak_sampled_named_temp_bytes'],total)
    except OSError as exc:spill_stats['errors'].append(type(exc).__name__+': '+str(exc))
    stop.wait(.2)
  observer=threading.Thread(target=observe,daemon=True);observer.start()
  def resource(label):
   p=subprocess.run(['docker','exec',name,'sh','-c','grep -E "^(Name|VmRSS|VmHWM|VmPeak|Threads):" /proc/1/status; cat /sys/fs/cgroup/memory.peak /sys/fs/cgroup/memory.max /sys/fs/cgroup/cpu.max; sha256sum /proc/1/exe'],capture_output=True,text=True)
   (phase/(label+'.resource.txt')).write_text(p.stdout+p.stderr)
  try:
   deadline=time.monotonic()+180;attempts=[]
   while True:
    logs=subprocess.run(['docker','logs',name],capture_output=True,text=True)
    if kind=='rdfdb' and 'statistics ready after' not in logs.stdout+logs.stderr:
     if time.monotonic()>deadline:raise TimeoutError('rdfdb statistics readiness')
     time.sleep(.2);continue
    try:
     fields=request_fields(kind,'SELECT (COUNT(*) AS ?count) WHERE { ?s ?p ?o }');raw=ask(endpoint,fields)
     count=json.loads(raw);assert int(count['results']['bindings'][0]['count']['value'])==10102032
     (phase/'graph-count.json').write_bytes(raw);break
    except (OSError, __import__('http.client',fromlist=['RemoteDisconnected']).RemoteDisconnected) as exc:
     attempts.append(type(exc).__name__+': '+str(exc))
     if time.monotonic()>deadline:raise
     time.sleep(.25)
   (phase/'readiness-attempts.json').write_text(json.dumps(attempts,indent=2));resource('before')
   if kind=='rdfdb':
    expected_binary=json.loads((s/'rdfdb-binary.json').read_text())['sha256'];assert hashlib.sha256((s/'target-linux/release/rdfdb').read_bytes()).hexdigest()==expected_binary
    assert expected_binary in (phase/'before.resource.txt').read_text()
   if kind=='duckdb':
    native=subprocess.check_output(['docker','exec',name,'python','-c','import duckdb,_duckdb,pathlib,hashlib,json; p=pathlib.Path(_duckdb.__file__); print(json.dumps({"version":duckdb.__version__,"package":duckdb.__file__,"native":str(p),"native_sha256":hashlib.sha256(p.read_bytes()).hexdigest()}))'],text=True)
    (phase/'loaded-duckdb.json').write_text(native);assert json.loads(native)['version']=='2.0.0'
   evidence={'system':kind,'endpoint':endpoint,'source_or_image':command[command.index('--entrypoint')+2],
    'dataset_sha256':manifest['source_data_sha256'],'threads':4,'memory_configuration':{'container_bytes':12*1024**3,'rdfdb_cache_mib':10240 if kind=='rdfdb' else None,'qlever_query_memory':'11 GB' if kind=='qlever' else None,'duckdb_working_memory':'8GB' if kind=='duckdb' else None,'duckdb_spill_limit':'6GB' if kind=='duckdb' else None,'virtuoso_number_of_buffers':1360000 if kind=='virtuoso' else None},
    'cache_profile':'no-answer-cache' if kind!='qlever' else ('full-bypass' if mode=='qlever-bypass' else 'clear-between-requests'),
    'query_fields':request_fields(kind,'') if kind=='virtuoso' else {}}
   evidence['query_fields'].pop('query',None)
   if kind=='qlever':
    settings=json.loads(ask(endpoint,{'cmd':'get-settings'}));(phase/'settings.json').write_text(json.dumps(settings,indent=2));assert settings['memory-for-queries']=='11 GB' and settings['max-num-threads-per-query']=='4'
    assert settings['disable-caching']==('true' if mode=='qlever-bypass' else 'false')
    assert settings['group-by-hash-map-enabled']==('true' if mode=='qlever-hash' else 'false')
    counters=json.loads(ask(endpoint,{'cmd':'clear-cache'}));(phase/'initial-clear.json').write_text(json.dumps(counters));assert len(counters)==5 and all(v==0 for v in counters.values())
    evidence.update(cache_control_verified=True,cache_control_evidence='vm-preflight-v1 plus live settings and initial-clear.json')
    if mode!='qlever-bypass':evidence['before_each_request']={'endpoint':endpoint,'fields':{'cmd':'clear-cache'},'expected_json':counters}
   elif kind=='rdfdb':evidence['source_commit']=json.loads((s/'rdfdb-source.json').read_text())['commit'];evidence['binary']=json.loads((s/'rdfdb-binary.json').read_text())
   (phase/'runtime-evidence.json').write_text(json.dumps(evidence,indent=2))
   args=[python,str(helper),'--workload',str(workload),'--reference',str(root/'bsbm10-holdout-v1/jena'),'--out',str(phase/'queries'),'--runtime-evidence',str(phase/'runtime-evidence.json'),'--numeric-inputs',str(root/'numeric-diagnostics-10m'),'--numeric-freeze-sha256','a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244','--count-policy-freeze',str(policy),'--count-policy-sha256',policy_sha,'--lock-owner',owner,'--timeout','120']
   (phase/'client-command.json').write_text(json.dumps(args,indent=2))
   with (phase/'client.log').open('wb') as dest:
    try:p=subprocess.run(args,stdout=dest,stderr=subprocess.STDOUT,timeout=1200);status[phase.name]={'client_exit':p.returncode}
    except subprocess.TimeoutExpired:status[phase.name]={'phase_timeout':1200}
   if (phase/'queries/measurements.json').exists():
    result=json.loads((phase/'queries/measurements.json').read_text());entry=status[phase.name];entry.update(complete=result['complete'],strict_pass=sum(q['status']=='pass' for q in result['queries'].values()),equivalent_pass=sum(q.get('count_equivalence',{}).get('status')=='pass' for q in result['queries'].values()),attempted=sum(q['status']!='not_run' for q in result['queries'].values()))
    message(f"Matched-VM BI {phase.name}: {entry['attempted']}/60 attempted; strict {entry['strict_pass']}, COUNT-equivalent {entry['equivalent_pass']} pass. Failed/missing outputs remain visible and their cohort totals are unavailable. Raw results and resources are retained; this is not yet a full cross-engine ranking.")
  except Exception as exc:
   status[phase.name]={'setup_error':type(exc).__name__+': '+str(exc)}
  finally:
   resource('after');stop.set();observer.join();(phase/'temp-observation.json').write_text(json.dumps(spill_stats,indent=2))
   logs=subprocess.run(['docker','logs',name],capture_output=True,text=True);text=logs.stdout+logs.stderr
   if env:text=text.replace(env['DBA_PASSWORD'],'<redacted>')
   (phase/'server.log').write_text(text)
   subprocess.run(['docker','stop','--timeout','3',name],capture_output=True)
   details=subprocess.run(['docker','inspect',name,'--format','{{json .State}}'],capture_output=True,text=True);(phase/'container-state.json').write_text(details.stdout+details.stderr)
   subprocess.run(['docker','rm',name],capture_output=True)
   (out/'status.json').write_text(json.dumps(status,indent=2))
print('MATCHED TWO-PASS COHORT FINISHED; no ranking inferred from failed results',flush=True)
