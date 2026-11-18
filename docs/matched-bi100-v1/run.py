"""Version-pinned serial, alternating BI100M protocol. Retains every failure."""
import hashlib,json,os,signal,subprocess,sys,time,urllib.request,urllib.parse,shutil
from pathlib import Path
stage=Path(__file__).resolve().parent;root=stage.parent
owner='Jianlin-Su task25 BI100M matched-VM ef7a778'
lock=Path('/tmp/rdf-bench.lock')
rust='rust@sha256:0b0ea86881537b3f645aace7e8e39556289f8787db057317c8d4978548213b9e'
python_image='python@sha256:94a8fee35252d06ab75856f15f12c7bae67b10e8a6da93c7fded82b2610926cd'
qlever='adfreiburg/qlever@sha256:d7ecda47b4d0132c190b4a73279761fd1c614f24cba4e894ca1ba8117cef6727'
rdf_store=Path('<agent-workspace>/rdfdb/bench/tools/rdfdb-disk-bsbm100-main')
duck_store=root/'duck100-import-v2/duck.db';ql_store=root/'qlever100-build-v1/index'
modules=root/'matched-vm-10m/duck-linux';reference=root/'bsbm100-reference-v1'
out=stage/'round1';out.mkdir(exist_ok=False)
protocol_raw=(stage/'protocol.json').read_bytes();protocol=json.loads(protocol_raw)
state={'complete':False,'phases':{},'started_unix':time.time(),'protocol_sha256':hashlib.sha256(protocol_raw).hexdigest()}
active_container=None;active_client=None
def sha(p):
    h=hashlib.sha256()
    with p.open('rb') as f:
        while b:=f.read(8*1024*1024):h.update(b)
    return h.hexdigest()
def bind(source,target,readonly=True):
    return ['--mount',f'type=bind,source={source},target={target}'+(',readonly' if readonly else '')]
def message(body):
    subprocess.run(['rtk','proxy','raft','task','reply','(internal discussion)'],input=body,text=True,capture_output=True)
def persist(): (out/'status.json').write_text(json.dumps(state,indent=2))
def assert_owner():assert (lock/'owner').read_text().strip()==owner
def inputs():
    rdf={str(p.relative_to(rdf_store)):{'size':p.stat().st_size,'mtime_ns':p.stat().st_mtime_ns,'sha256':sha(p)} for p in sorted(rdf_store.rglob('*')) if p.is_file()}
    ql={str(p.relative_to(ql_store)):{'bytes':p.stat().st_size,'sha256':sha(p)} for p in sorted(ql_store.rglob('*')) if p.is_file()}
    duck=sha(duck_store)
    assert rdf==json.loads((root/'bi100-rdfdb-qualification-v1/store-after.json').read_text())
    assert ql==json.loads((root/'qlever100-build-v1/status.json').read_text())['index_files']
    assert duck=='1ad0939f1aead75a8b21f157e349f5a8d1db527305d4d0424c5c014d87aa8f9d'
    return {'rdfdb':rdf,'qlever':ql,'duckdb':duck}
def ask(endpoint,fields,timeout=10):
    req=urllib.request.Request(endpoint,data=urllib.parse.urlencode(fields).encode(),headers={'Content-Type':'application/x-www-form-urlencoded','Accept':'application/sparql-results+json'})
    with urllib.request.urlopen(req,timeout=timeout) as response:return json.load(response)
def logs(name):
    r=subprocess.run(['docker','logs',name],capture_output=True);return r.stdout+r.stderr
def cleanup(name,phase):
    inspect=subprocess.run(['docker','inspect',name,'--format','{{json .State}}'],capture_output=True,text=True)
    if inspect.returncode==0:
        value=json.loads(inspect.stdout);(phase/'container-state-before-stop.json').write_text(json.dumps(value,indent=2))
        (phase/'server.log').write_bytes(logs(name))
        if value.get('Running'):
            resource=subprocess.run(['docker','exec',name,'cat','/sys/fs/cgroup/memory.peak','/sys/fs/cgroup/memory.max','/sys/fs/cgroup/memory.swap.max'],capture_output=True,text=True)
            (phase/'cgroup-memory.json').write_text(json.dumps({'command_scope':'container cgroup memory.peak, memory.max, memory.swap.max; not process RSS','exit_code':resource.returncode,'stdout':resource.stdout,'stderr':resource.stderr},indent=2))
            subprocess.run(['docker','stop','--time','15',name],check=True,capture_output=True)
        final=subprocess.run(['docker','inspect',name,'--format','{{json .State}}'],capture_output=True,text=True)
        (phase/'container-state-after-stop.json').write_text(final.stdout)
        subprocess.run(['docker','rm',name],check=True,capture_output=True)
def interrupted(signum,frame):raise KeyboardInterrupt('runner signal '+str(signum))
signal.signal(signal.SIGTERM,interrupted)
try:
    assert_owner();assert (stage/'protocol.json').read_bytes()==protocol_raw
    assert sha(stage/'target-linux-ef7a778/release/rdfdb')==json.loads((stage/'rdfdb-linux-binary.json').read_text())['sha256']
    before=inputs();(out/'input-hashes-before.json').write_text(json.dumps(before,indent=2))
    module_hashes={str(p.relative_to(modules)):sha(p) for p in sorted(modules.rglob('*')) if p.is_file() and '__pycache__' not in p.parts}
    (out/'duck-module-hashes.json').write_text(json.dumps(module_hashes,indent=2))
    (out/'protocol.json').write_bytes(protocol_raw)
    for pass_number,systems in enumerate(protocol['passes'],1):
        for system in systems:
            assert_owner();assert (stage/'protocol.json').read_bytes()==protocol_raw
            used=int(subprocess.check_output(['du','-sk',str(root)],text=True).split()[0])*1024
            assert used<40*1024**3 and shutil.disk_usage(root).free>48*1024**3
            phase=out/f'pass{pass_number}-{system}';phase.mkdir();temporary=phase/'tmp';temporary.mkdir()
            entry={'complete':False,'started_unix':time.time()};state['phases'][phase.name]=entry;persist()
            name=f'rdf-jianlin-bi100-r1-p{pass_number}-{system}'
            active_container=name;active_phase=phase
            try:
                port={'rdfdb':31441,'duckdb':31442,'qlever':31443}[system]
                endpoint=f'http://127.0.0.1:{port}/'+('' if system=='qlever' else 'sparql')
                command=['docker','run','-d','--name',name,'--cpus','4','--memory','12g','--memory-swap','12g','-p',f'127.0.0.1:{port}:{port}']
                command+=bind(stage,'/work')+bind(temporary,'/tmp',False)+['-e','TMPDIR=/tmp']
                if system=='rdfdb':
                    command+=bind(rdf_store,'/data')+['-e','RAYON_NUM_THREADS=4','-e','RDFDB_HOST=0.0.0.0','--entrypoint','/work/target-linux-ef7a778/release/rdfdb',rust,'serve','/data',str(port),'--disk','--cache-mb=10240']
                    image=rust
                elif system=='duckdb':
                    command+=bind(duck_store,'/data/duck.db')+bind(modules,'/modules')+bind(lock,'/tmp/rdf-bench.lock')+['-e','PYTHONPATH=/modules:/work/bench-source','-e','PYTHONDONTWRITEBYTECODE=1','--entrypoint','python',python_image,'/work/bench-source/duck_http.py','--workload','/work/workload','--database','/data/duck.db','--import-report','/work/duck-import.json','--spill','/tmp/spill','--lock-owner',owner,'--host','0.0.0.0','--port',str(port),'--threads','4','--memory-limit','8GB','--max-spill','8GB']
                    image=python_image
                else:
                    command+=bind(ql_store,'/index')+['--workdir','/index','--entrypoint','/qlever/qlever-server',qlever,'-i','bsbm100','-p',str(port),'-m','11GB','-c','1MB','-e','1MB','-E','1MB','-s','600s','--set-runtime-parameter','max-num-threads-per-query=4','--no-metrics-log','--no-resource-usage-log']
                    image=qlever
                (phase/'server-command.json').write_text(json.dumps(command,indent=2))
                (phase/'container-id').write_text(subprocess.check_output(command,text=True).strip())
                deadline=time.monotonic()+600
                while True:
                    inspect=json.loads(subprocess.check_output(['docker','inspect',name,'--format','{{json .State}}'],text=True))
                    if not inspect['Running']:raise RuntimeError('Server exited before readiness')
                    raw=logs(name).decode(errors='replace')
                    if system=='rdfdb':ready='statistics ready after' in raw
                    elif system=='duckdb':ready='"system": "duckdb"' in raw and '"version": "2.0.0"' in raw
                    else:
                        try:ready=ask(endpoint,{'query':'ASK {}'})['boolean'] is True
                        except Exception:ready=False
                    if ready:break
                    if time.monotonic()>deadline:raise TimeoutError('Server readiness')
                    time.sleep(.3)
                (phase/'readiness.log').write_text(raw)
                evidence={'system':system,'endpoint':endpoint,'source_or_image':image,'dataset_sha256':protocol['dataset_sha256'],'threads':4,
                  'memory_configuration':{'container_bytes':12*1024**3,'extra_swap_bytes':0,'engine_budget':{'rdfdb':'10240 MB shared cache','duckdb':'8GB working / 8GB spill','qlever':'11GB query memory'}[system]},
                  'cache_profile':'normal engine caches, no result cache' if system!='qlever' else 'clear-between-requests'}
                if system=='rdfdb':evidence.update(source_commit=protocol['rdfdb_commit'],binary=json.loads((stage/'rdfdb-linux-binary.json').read_text()),statistics_ready=True)
                if system=='qlever':
                    settings=ask(endpoint,{'cmd':'get-settings'});(phase/'settings.json').write_text(json.dumps(settings,indent=2))
                    assert settings['memory-for-queries']=='11 GB' and settings['max-num-threads-per-query']=='4' and settings['group-by-hash-map-enabled']=='false' and settings['disable-caching']=='false'
                    counters=ask(endpoint,{'cmd':'clear-cache'});assert len(counters)==5 and all(v==0 for v in counters.values())
                    (phase/'initial-clear.json').write_text(json.dumps(counters,indent=2))
                    evidence.update(cache_control_verified=True,cache_control_evidence='live settings and five zero-valued counters; per-request acknowledgements retained',before_each_request={'endpoint':endpoint,'fields':{'cmd':'clear-cache'},'expected_json':counters})
                (phase/'runtime-evidence.json').write_text(json.dumps(evidence,indent=2))
                args=[sys.executable,str(stage/'bench-source/http_measure_value.py'),'--workload',str(stage/'workload'),'--reference',str(reference/'reference'),'--out',str(phase/'queries'),'--runtime-evidence',str(phase/'runtime-evidence.json'),'--numeric-inputs',str(root/'numeric-freeze-100m-v1'),'--numeric-freeze-sha256',protocol['gates']['numeric_freeze'],'--count-policy-freeze',str(reference/'count-policy-freeze.json'),'--count-policy-sha256',protocol['gates']['count_policy'],'--value-policy-freeze',str(reference/'value-policy-freeze.json'),'--value-policy-sha256',protocol['gates']['value_policy'],'--lock-owner',owner,'--timeout','600']
                guarded=[sys.executable,str(stage/'bench-source/supervise_import.py'),'--root',str(root),'--receipt',str(phase/'supervisor.json'),'--owner',owner,'--',*args]
                (phase/'client-command.json').write_text(json.dumps(guarded,indent=2))
                with (phase/'client.log').open('wb') as log:
                    active_client=subprocess.Popen(guarded,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
                    entry['client_pid']=active_client.pid;entry['client_exit']=active_client.wait();active_client=None
                report_path=phase/'queries/measurements.json'
                if report_path.exists():
                    report=json.loads(report_path.read_text());entry.update(complete=report['complete'],attempted=sum(q['status']!='not_run' for q in report['queries'].values()),
                      primary_pass=sum(q['status']=='pass' for q in report['queries'].values()),count_pass=sum(q.get('count_equivalence',{}).get('status')=='pass' for q in report['queries'].values()),value_pass=sum(q.get('value_compatibility',{}).get('status')=='pass' for q in report['queries'].values()))
                message(f'BI100M {phase.name}: {entry.get("attempted",0)}/60 attempted, primary {entry.get("primary_pass",0)}, COUNT-compatible {entry.get("count_pass",0)}, value-compatible {entry.get("value_pass",0)} passed. Raw failures and missing requests remain retained; final comparison follows both passes.')
            except Exception as exc:entry['error']=type(exc).__name__+': '+str(exc)
            finally:
                if active_client is not None and active_client.poll() is None:active_client.terminate();active_client.wait();active_client=None
                cleanup(name,phase);active_container=None;entry['ended_unix']=time.time();persist()
    after=inputs();(out/'input-hashes-after.json').write_text(json.dumps(after,indent=2));assert before==after
    for n,h in module_hashes.items():assert sha(modules/n)==h
    state['complete']=len(state['phases'])==6 and all(p['complete'] for p in state['phases'].values())
    state['inputs_and_modules_unchanged']=True
except BaseException as exc:state['error']=type(exc).__name__+': '+str(exc)
finally:
    if active_client is not None and active_client.poll() is None:active_client.terminate();active_client.wait()
    if active_container is not None:cleanup(active_container,active_phase)
    state['ended_unix']=time.time();persist()
    if (lock/'owner').exists() and (lock/'owner').read_text().strip()==owner:(lock/'owner').unlink();lock.rmdir()
    message('Machine free: the BI100M matched-VM runner has ended and its owned containers and clients have exited. I will audit complete/failed cohorts, hashes and both passes before reporting a speed ratio.')
