"""Registered aggregate72 same-VM run; retain both orders and every failure."""
import hashlib,json,os,re,signal,subprocess,sys,time,urllib.parse,urllib.request
from pathlib import Path
stage=Path(__file__).resolve().parent;root=stage.parent
owner='Jianlin task25 aggregate72 matched VM disk v1'
protocol_raw=(stage/'protocol.json').read_bytes();protocol=json.loads(protocol_raw)
sys.path.insert(0,str(stage/'bench-source'))
from aggregate_count_policy import compare
prep=root/'aggregate72-preparation-v1';reference=root/'aggregate72-reference-v2'
rules=json.loads((prep/'rules-freeze.json').read_text());refs=json.loads((reference/'reference-binding.json').read_text())
out=stage/'round1';out.mkdir(exist_ok=False)
state={'complete':False,'started':time.time(),'phases':{},'protocol_sha256':hashlib.sha256(protocol_raw).hexdigest()}
active=None;phase=None;active_system=None

def save():
    (out/'status.json').write_text(json.dumps(state,indent=2))

def sha(path):
    h=hashlib.sha256()
    with Path(path).open('rb') as stream:
        while b:=stream.read(8*1024*1024):h.update(b)
    return h.hexdigest()

def files(folder):
    return {str(f.relative_to(folder)):{'bytes':f.stat().st_size,'sha256':sha(f)} for f in sorted(folder.rglob('*')) if f.is_file()}

def assert_owner():
    assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
    assert (stage/'protocol.json').read_bytes()==protocol_raw
    for name,digest in protocol['source_files'].items():assert sha(stage/name)==digest

def request(endpoint,fields=None,timeout=610):
    data=None if fields is None else urllib.parse.urlencode(fields).encode()
    req=urllib.request.Request(endpoint,data=data,headers={'Accept':'application/sparql-results+json','Content-Type':'application/x-www-form-urlencoded','Cache-Control':'no-cache','Accept-Encoding':'identity'})
    with urllib.request.urlopen(req,timeout=timeout) as response:return response.read()

def graph_count(system,dataset,endpoint,label):
    if system=='duckdb':
        raw=request(endpoint.rsplit('/',1)[0]+'/health',None,30);(phase/('graph-count-'+label+'.json')).write_bytes(raw)
        return int(json.loads(raw)['rows'])
    fields={'query':'SELECT (COUNT(*) AS ?n) WHERE {?s ?p ?o}'}
    if system=='virtuoso':fields.update({'default-graph-uri':'http://bench/aggregate72/'+dataset,'format':'application/sparql-results+json'})
    raw=request(endpoint,fields);(phase/('graph-count-'+label+'.json')).write_bytes(raw)
    return int(json.loads(raw)['results']['bindings'][0]['n']['value'])

def cleanup(name,folder,system):
    inspect=subprocess.run(['docker','inspect',name,'--format','{{json .State}}'],capture_output=True,text=True)
    if inspect.returncode:
        (folder/'container-inspect-error.txt').write_text(inspect.stderr);return
    current=json.loads(inspect.stdout);(folder/'container-before-stop.json').write_text(inspect.stdout)
    if current['Running']:
        peak=subprocess.run(['docker','exec',name,'cat','/sys/fs/cgroup/memory.peak','/sys/fs/cgroup/memory.max','/sys/fs/cgroup/memory.swap.max'],capture_output=True,text=True)
        (folder/'cgroup-memory.json').write_text(json.dumps({'metric':'container cgroup memory.peak, not process RSS; includes startup and warmup','exit_code':peak.returncode,'stdout':peak.stdout,'stderr':peak.stderr},indent=2))
        subprocess.run(['docker','stop','--time','10',name],capture_output=True,check=True)
    logs=subprocess.run(['docker','logs',name],capture_output=True);raw=logs.stdout+logs.stderr
    if system=='virtuoso':raw=b''.join(b'[REDACTED credential-related diagnostic line]\n' if re.search(rb'(?i)(password|\bpwd\b|secret|credential|\btoken\b)',line) else line for line in raw.splitlines(keepends=True))
    (folder/'server.log').write_bytes(raw)
    final=subprocess.run(['docker','inspect',name,'--format','{{json .State}}'],capture_output=True,text=True,check=True);(folder/'container-after-stop.json').write_text(final.stdout)
    assert not json.loads(final.stdout)['Running']
    subprocess.run(['docker','rm',name],capture_output=True,check=True)

def integrity(system,dataset):
    config=protocol['datasets'][dataset]
    if system=='rdfdb':
        actual=files(Path(config['rdfdb_store']));assert actual==config['rdfdb_files'];return actual
    if system=='qlever':
        actual=files(Path(config['qlever_store']));assert actual==config['qlever_files'];return actual
    if system=='duckdb':
        actual=sha(config['duckdb_store']);assert actual==config['duckdb_sha256'];return actual
    # Virtuoso's owned database has administrative writes on reopen/checkpoint.
    # Its source binding, selected INI settings, full graph counts and every query are checked separately.
    return {'dataset_sha256':config['source_sha256'],'store':config['virtuoso_store'],'byte_immutable':False}

def interrupted(signum,frame):raise KeyboardInterrupt('owned benchmark interrupted')
signal.signal(signal.SIGTERM,interrupted)

try:
    assert_owner();assert sha(prep/'rules-freeze.json')==protocol['rules_sha256']
    assert sha(reference/'reference-binding.json')==protocol['reference_binding_sha256']
    assert sha(stage/'target-linux-9a2f49b/release/rdfdb')==protocol['rdfdb_binary_sha256']
    for relative,digest in protocol['duck_module_files'].items():assert sha(Path(protocol['duck_modules'])/relative)==digest
    for dataset,config in protocol['datasets'].items():assert sha(config['source_path'])==config['source_sha256']
    for pass_number,systems in enumerate(protocol['engine_orders'],1):
        for dataset in protocol['dataset_orders'][pass_number-1]:
            config=protocol['datasets'][dataset]
            entries=[(name,r) for name,r in rules['queries'].items() if r['dataset']==dataset]
            for system in systems:
                assert_owner();key=f'pass{pass_number}-{dataset}-{system}';phase=out/key;phase.mkdir()
                name='rdf-jianlin-agg72-match-'+key;active=name;active_system=system
                entry={'complete':False,'requests':{},'system':system,'dataset':dataset,'pass':pass_number};state['phases'][key]=entry;save()
                before=integrity(system,dataset);(phase/'store-before.json').write_text(json.dumps(before,indent=2))
                port={'rdfdb':31461,'duckdb':31462,'virtuoso':31463,'qlever':31464}[system]
                endpoint=f'http://127.0.0.1:{port}/'+('' if system=='qlever' else 'sparql')
                image=protocol['images'][system]
                cmd=['docker','run','--detach','--name',name,'--cpus=4','--memory=12g','--memory-swap=12g']
                cmd+=['-p',f'127.0.0.1:{port}:'+str(8890 if system=='virtuoso' else port)]
                def mount(source,target,readonly=True):return ['--mount','type=bind,source='+str(source)+',target='+target+(',readonly' if readonly else '')]
                if system=='rdfdb':
                    cmd+=mount(stage/'target-linux-9a2f49b/release/rdfdb','/app/rdfdb')+mount(config['rdfdb_store'],'/data')
                    cmd+=['--env','RAYON_NUM_THREADS=4','--env','RDFDB_HOST=0.0.0.0','--entrypoint','/app/rdfdb',image,'serve','/data',str(port),'--disk','--cache-mb=10240']
                elif system=='duckdb':
                    (phase/'spill').mkdir();cmd+=mount(config['duckdb_store'],'/data/duck.db')+mount(protocol['duck_modules'],'/modules')+mount(stage/'bench-source','/source')+mount(prep/'rules-freeze.json','/rules.json')+mount(prep/'workload','/workload')+mount(phase,'/phase',False)
                    cmd+=['--env','PYTHONPATH=/modules:/source','--env','PYTHONDONTWRITEBYTECODE=1','--entrypoint','python',image,'/source/duck_http.py','--database','/data/duck.db','--workload','/workload','--rules','/rules.json','--dataset',dataset,'--spill','/phase/spill','--port',str(port)]
                elif system=='qlever':
                    cmd+=mount(config['qlever_store'],'/index')+['--workdir','/index','--entrypoint','/qlever/qlever-server',image,'-i',dataset+'100','-p',str(port),'-m','11GB','-c','1MB','-e','1MB','-E','1MB','-s','600s','--set-runtime-parameter','max-num-threads-per-query=4','--no-metrics-log','--no-resource-usage-log']
                else:
                    # Existing owned database/INI; no credential or administrative SQL is needed for readonly SPARQL.
                    cmd+=mount(config['virtuoso_store'],'/database',False)+[image]
                (phase/'server-command.json').write_text(json.dumps(cmd,indent=2));entry['container_id']=subprocess.check_output(cmd,text=True).strip();save()
                resource_format='{"cpu_nanos":{{.HostConfig.NanoCpus}},"memory":{{.HostConfig.Memory}},"memory_swap_total":{{.HostConfig.MemorySwap}},"image_id":"{{.Image}}"}'
                actual_resources=json.loads(subprocess.check_output(['docker','inspect',name,'--format',resource_format],text=True));(phase/'actual-resources.json').write_text(json.dumps(actual_resources,indent=2))
                assert actual_resources=={'cpu_nanos':4000000000,'memory':12*1024**3,'memory_swap_total':12*1024**3,'image_id':protocol['image_ids'][system]}

                try:
                    deadline=time.monotonic()+600
                    while True:
                        current=json.loads(subprocess.check_output(['docker','inspect',name,'--format','{{json .State}}'],text=True));assert current['Running'],'server exited before readiness'
                        ready=False
                        if system=='rdfdb':
                            logs=subprocess.run(['docker','logs',name],capture_output=True);ready=b'statistics ready after' in logs.stdout+logs.stderr
                        else:
                            try:
                                if system=='duckdb':
                                    health=json.loads(request(f'http://127.0.0.1:{port}/health',None,10));ready=health['version']=='2.0.0'
                                else:
                                    fields={'query':'ASK {}'}
                                    if system=='virtuoso':fields.update({'default-graph-uri':'http://bench/aggregate72/'+dataset,'format':'application/sparql-results+json'})
                                    ready=json.loads(request(endpoint,fields,10)).get('boolean') is True
                            except Exception:pass
                        if ready:break
                        if time.monotonic()>deadline:raise TimeoutError('server readiness expired')
                        time.sleep(.3)
                    if system=='qlever':
                        settings=json.loads(request(endpoint,{'cmd':'get-settings'}));(phase/'settings.json').write_text(json.dumps(settings,indent=2));assert settings['memory-for-queries']=='11 GB' and settings['max-num-threads-per-query']=='4' and settings['group-by-hash-map-enabled']=='false' and settings['disable-caching']=='false'
                    entry['graph_count_before']=graph_count(system,dataset,endpoint,'before');assert entry['graph_count_before']==config['graph_count'];save()
                    for part in ['warmup','measured']:
                        folder=phase/part;folder.mkdir()
                        for qname,record in entries:
                            query=prep/'workload'/qname;assert sha(query)==record['query_sha256'];stem=Path(qname).stem
                            if system=='qlever':
                                clear=json.loads(request(endpoint,{'cmd':'clear-cache'}));assert len(clear)==5 and all(v==0 for v in clear.values());(folder/(stem+'-clear.json')).write_text(json.dumps(clear))
                            fields={'query':query.read_text()}
                            if system=='virtuoso':fields.update({'default-graph-uri':'http://bench/aggregate72/'+dataset,'format':'application/sparql-results+json'})
                            req=urllib.request.Request(endpoint,data=urllib.parse.urlencode(fields).encode(),headers={'Accept':'application/sparql-results+json','Content-Type':'application/x-www-form-urlencoded','Cache-Control':'no-cache','Accept-Encoding':'identity'})
                            start=time.perf_counter_ns()
                            with urllib.request.urlopen(req,timeout=610) as response:
                                body=response.read();end=time.perf_counter_ns();http_headers={key:response.headers.get(key) for key in ['Content-Type','Content-Length','Content-Encoding','Transfer-Encoding']}
                            elapsed=(end-start)/1e9
                            actual_file=folder/(stem+'.json');actual_file.write_bytes(body);ref=reference/dataset/(stem+'.json');assert sha(ref)==refs['references'][qname]['sha256']
                            result=compare(json.loads(body),json.loads(ref.read_text()),record['columns']);result.update(elapsed_seconds=elapsed,query_sha256=record['query_sha256'],body_sha256=sha(actual_file),body_bytes=len(body),part=part,http_headers=http_headers)
                            entry['requests'][part+'/'+qname]=result;save()
                            if not result['count_compatible_pass']:raise RuntimeError('Frozen COUNT-compatible gate failed; retain complete attempted outputs')
                    entry['graph_count_after']=graph_count(system,dataset,endpoint,'after');assert entry['graph_count_after']==config['graph_count'];entry['complete']=True;save()
                finally:
                    cleanup(active,phase,system);active=None
                    after=integrity(system,dataset);(phase/'store-after.json').write_text(json.dumps(after,indent=2));entry['immutable_store_check_pass']=before==after;save()
    for dataset,config in protocol['datasets'].items():assert sha(config['source_path'])==config['source_sha256']
    for relative,digest in protocol['duck_module_files'].items():assert sha(Path(protocol['duck_modules'])/relative)==digest
    assert_owner();state['complete']=len(state['phases'])==16 and all(e['complete'] for e in state['phases'].values())
except BaseException as exc:
    state['error']=type(exc).__name__+': '+str(exc)
    raise
finally:
    if active is not None:cleanup(active,phase,active_system)
    state['ended']=time.time();save()
