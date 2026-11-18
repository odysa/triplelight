import hashlib,json,subprocess,sys,os,time,signal,urllib.request,urllib.parse
from pathlib import Path
root=Path('/private/tmp/rdf-jianlin-analytical25');p=Path(__file__).resolve().parent;prep=root/'aggregate72-preparation-v1';build=root/'aggregate72-qlever-build-v1';owner='Jianlin task25 aggregate72 QLever qualification v1'
sys.path.insert(0,str(prep/'source'));from aggregate_count_policy import compare
f=json.loads((prep/'rules-freeze.json').read_text());refs=json.loads((root/'aggregate72-reference-v2/reference-binding.json').read_text());image='adfreiburg/qlever@sha256:d7ecda47b4d0132c190b4a73279761fd1c614f24cba4e894ca1ba8117cef6727';endpoint='http://127.0.0.1:31448/'
def sha(file):
 h=hashlib.sha256()
 with Path(file).open('rb') as stream:
  while b:=stream.read(8*1024*1024):h.update(b)
 return h.hexdigest()
def indices(folder):return {str(q.relative_to(folder)):{'bytes':q.stat().st_size,'sha256':sha(q)} for q in sorted(folder.rglob('*')) if q.is_file()}
def ask(fields,timeout=610):
 req=urllib.request.Request(endpoint,data=urllib.parse.urlencode(fields).encode(),headers={'Accept':'application/sparql-results+json','Content-Type':'application/x-www-form-urlencoded'})
 with urllib.request.urlopen(req,timeout=timeout) as response:return response.read()
def cleanup(name,d):
 check=subprocess.run(['docker','inspect',name,'--format','{{json .State}}'],capture_output=True,text=True)
 if check.returncode==0:
  state=json.loads(check.stdout);(d/'container-before-cleanup.json').write_text(check.stdout)
  logs=subprocess.run(['docker','logs',name],capture_output=True);(d/'server.log').write_bytes(logs.stdout+logs.stderr)
  if state['Running']:subprocess.run(['docker','stop','--time','5',name],check=True,capture_output=True)
  final=subprocess.run(['docker','inspect',name,'--format','{{json .State}}'],check=True,capture_output=True,text=True);(d/'container-after-stop.json').write_text(final.stdout);assert not json.loads(final.stdout)['Running']
  subprocess.run(['docker','rm',name],check=True,capture_output=True)
def interrupted(signum,frame):raise KeyboardInterrupt('owned qualification interrupted')
signal.signal(signal.SIGTERM,interrupted)
s={'complete':False,'datasets':{},'comparative_timing_claim':False,'image':image,'started':time.time()};active=None;d=None
def save():(p/'status.json').write_text(json.dumps(s,indent=2))
try:
 assert sha(prep/'rules-freeze.json')=='5cf92a76e599d1e8a1db1f56a3f5215c3b05fea989665754c02ce3f76bb7f365'
 for dataset in ['lubm','watdiv']:
  assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
  d=p/dataset;d.mkdir();folder=build/dataset/'index';built=json.loads((build/dataset/'status.json').read_text());assert built['complete'] and built['input_before']==built['input_after'] and built['input_before']['sha256']==f['inputs'][dataset]['sha256']
  assert built['metadata']['has-all-permutations'];before=indices(folder);assert before==built['index_files'];(d/'index-before.json').write_text(json.dumps(before,indent=2))
  name='rdf-jianlin-agg72-ql-'+dataset+'-qual-v1';active=name
  cmd=['docker','run','--detach','--name',name,'--cpus=4','--memory=12g','--memory-swap=12g','-p','127.0.0.1:31448:31448','--mount','type=bind,source='+str(folder)+',target=/index,readonly','--workdir','/index','--entrypoint','/qlever/qlever-server',image,'-i',dataset+'100','-p','31448','-m','11GB','-c','1MB','-e','1MB','-E','1MB','-s','600s','--set-runtime-parameter','max-num-threads-per-query=4','--no-metrics-log','--no-resource-usage-log']
  entry={'queries':{},'command':cmd};s['datasets'][dataset]=entry;save();entry['container_id']=subprocess.check_output(cmd,text=True).strip();save()
  try:
   deadline=time.monotonic()+120
   while True:
    try:
     if json.loads(ask({'query':'ASK {}'},10))['boolean'] is True:break
    except Exception:pass
    if time.monotonic()>deadline:raise TimeoutError('QLever readiness expired')
    time.sleep(.3)
   settings=json.loads(ask({'cmd':'get-settings'}));(d/'settings.json').write_text(json.dumps(settings,indent=2));assert settings['memory-for-queries']=='11 GB' and settings['max-num-threads-per-query']=='4'
   (d/'dataset-count.json').write_bytes(ask({'query':'SELECT (COUNT(*) AS ?n) WHERE {?s ?p ?o}'}))
   for name,record in f['queries'].items():
    if record['dataset']!=dataset:continue
    q=prep/'workload'/name;assert sha(q)==record['query_sha256'];clear=json.loads(ask({'cmd':'clear-cache'}));assert len(clear)==5 and all(v==0 for v in clear.values());(d/(Path(name).stem+'-clear.json')).write_text(json.dumps(clear))
    raw=ask({'query':q.read_text()});out=d/(Path(name).stem+'.json');out.write_bytes(raw);ref=root/'aggregate72-reference-v2'/dataset/(Path(name).stem+'.json');assert sha(ref)==refs['references'][name]['sha256']
    result=compare(json.loads(raw),json.loads(ref.read_text()),record['columns']);result['output_sha256']=sha(out);entry['queries'][name]=result;save()
  except Exception as exc:entry['error']=repr(exc)
  finally:
   cleanup(active,d);active=None;after=indices(folder);(d/'index-after.json').write_text(json.dumps(after,indent=2));entry['index_unchanged']=before==after;save()
 s['complete']=len(s['datasets'])==2 and all(len(e['queries'])==sum(r['dataset']==n for r in f['queries'].values()) and not e.get('error') and e['index_unchanged'] for n,e in s['datasets'].items())
finally:
 if active is not None:cleanup(active,d)
 s['ended']=time.time();save()
