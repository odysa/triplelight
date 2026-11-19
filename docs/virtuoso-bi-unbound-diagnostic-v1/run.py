import collections,hashlib,json,os,subprocess,sys,time,urllib.request,urllib.parse
from pathlib import Path
p=Path(__file__).resolve().parent;root=p.parent;owner='Jianlin task25 Virtuoso BI q5 diagnosis'
assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
sys.path.insert(0,str(root/'aggregate72-matched-v1/bench-source'));from qualify import exact_term
image='openlink/virtuoso-opensource-7@sha256:0dbe1ab4fa0cb7bbafc1f6c0c2b0a5d6f22d918dbd17672f2ddb24580aa6756a'
name='rdf-jianlin-virt-bi-diag-v1';store=root/'matched-vm-10m/virtuoso-v3'
command=['docker','run','-d','--name',name,'--cpus','4','--memory','12g','--memory-swap','12g','-p','127.0.0.1:31465:8890','-v',str(store)+':/database',image]
(p/'server-command.json').write_text(json.dumps(command,indent=2));results={};commands={}
def save(name,x):(p/name).write_text(json.dumps(x,indent=2)+'\n')
def query(q,graph=True,timeout=130):
 data={'query':q,'format':'application/sparql-results+json'}
 if graph:data['default-graph-uri']='http://bench/bsbm10'
 request=urllib.request.Request('http://127.0.0.1:31465/sparql',data=urllib.parse.urlencode(data).encode(),headers={'Accept':'application/sparql-results+json','Cache-Control':'no-cache','Accept-Encoding':'identity'})
 with urllib.request.urlopen(request,timeout=timeout) as response:return response.read()
def bag(obj):return collections.Counter(tuple((n,exact_term(t)) for n,t in sorted(row.items())) for row in obj['results']['bindings'])
def keybag(obj):return collections.Counter(tuple((n,exact_term(t)) for n,t in sorted(row.items()) if n in ['country','product']) for row in obj['results']['bindings'])
try:
 proc=subprocess.run(command,capture_output=True,text=True);save('start-result.json',{'exit':proc.returncode,'stdout':proc.stdout,'stderr':proc.stderr});assert proc.returncode==0
 for i in range(90):
  try:
   raw=query('ASK {}',False,10);assert json.loads(raw)['boolean'];break
  except Exception:time.sleep(1)
 else:raise RuntimeError('readiness failed')
 save('resources.json',json.loads(subprocess.check_output(['docker','inspect',name,'--format','{{json .HostConfig}}'])) | {})
 # HostConfig contains names/binds but no environment values; retain only selected limits.
 resources=json.loads((p/'resources.json').read_text());save('resources.json',{k:resources[k] for k in ['NanoCpus','Memory','MemorySwap']});assert resources['NanoCpus']==4000000000 and resources['Memory']==12*1024**3 and resources['MemorySwap']==12*1024**3
 raw=query('SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }');(p/'graph-count-before.json').write_bytes(raw);assert int(json.loads(raw)['results']['bindings'][0]['n']['value'])==10102032
 jena=Path('<agent-workspace>/rdfdb/bench/tools/apache-jena-6.2.0')
 for qfile in sorted((p/'controls').glob('*.rq')):
  cmd=[str(jena/'bin/arq'),'--data='+str(p/'fixture.nt'),'--query='+str(qfile),'--results=JSON'];proc=subprocess.run(cmd,capture_output=True,timeout=40,env={**os.environ,'JVM_ARGS':'-Xmx1g -XX:ActiveProcessorCount=1'})
  commands[qfile.stem]={'jena_command':cmd,'exit':proc.returncode};qfile.with_name(qfile.stem+'-jena.json').write_bytes(proc.stdout);qfile.with_name(qfile.stem+'-jena.stderr').write_bytes(proc.stderr);assert proc.returncode==0
  actual=query(qfile.read_text(),False);qfile.with_name(qfile.stem+'-virtuoso.json').write_bytes(actual);ref=json.loads(proc.stdout);got=json.loads(actual)
  results[qfile.stem]={'reference_rows':len(ref['results']['bindings']),'actual_rows':len(got['results']['bindings']),'exact_term_bag_pass':bag(got)==bag(ref),'country_product_bag_pass':keybag(got)==keybag(ref)};save('results.json',results);save('jena-commands.json',commands)
 ref=json.loads((p/'q5-reference.json').read_text())
 for stem in ['q5-original','q5-remove-unbound']:
  raw=query((p/(stem+'.rq')).read_text());(p/(stem+'-virtuoso.json')).write_bytes(raw);got=json.loads(raw)
  results[stem]={'reference_rows':len(ref['results']['bindings']),'actual_rows':len(got['results']['bindings']),'exact_term_bag_pass':bag(got)==bag(ref),'country_product_bag_pass':keybag(got)==keybag(ref)};save('results.json',results)
 raw=query('SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }');(p/'graph-count-after.json').write_bytes(raw);assert int(json.loads(raw)['results']['bindings'][0]['n']['value'])==10102032
 save('status.json',{'complete':True,'ended':time.time()});print(json.dumps(results,indent=2),flush=True)
except BaseException as e:
 save('status.json',{'complete':False,'error':repr(e),'ended':time.time()});raise
finally:
 proc=subprocess.run(['docker','stop','-t','10',name],capture_output=True,text=True);save('stop.json',{'exit':proc.returncode,'stdout':proc.stdout,'stderr':proc.stderr})
 proc=subprocess.run(['docker','logs',name],capture_output=True);raw=proc.stdout+proc.stderr
 # Never persist credential diagnostic lines.
 clean=b'\n'.join(b'[credential diagnostic redacted]' if any(token in line.lower() for token in [b'password',b'credential',b'secret']) else line for line in raw.splitlines());(p/'server.log').write_bytes(clean)
 proc=subprocess.run(['docker','inspect',name,'--format','{{json .State}}'],capture_output=True,text=True)
 if proc.returncode==0:save('container-after-stop.json',json.loads(proc.stdout))
 proc=subprocess.run(['docker','rm',name],capture_output=True,text=True);save('remove.json',{'exit':proc.returncode,'stdout':proc.stdout,'stderr':proc.stderr})
