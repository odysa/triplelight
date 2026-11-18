from pathlib import Path
import os,json,time,hashlib,socket,signal,subprocess,urllib.request,urllib.parse,shutil
root=Path(__file__).parent;out=root/'numeric-diagnostics-100m-v1'/'jena-run-v1'
owner='Jianlin-Su task25 100M independent Jena numeric counts'
assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
assert shutil.disk_usage('/System/Volumes/Data').free>40*1024**3
out.mkdir();shutil.copyfile(Path(__file__),out/'supervisor-source.py')
tools=Path('<agent-workspace>/rdfdb/bench/tools')
store=tools/'tdb2-bsbm100';program=tools/'apache-jena-fuseki-6.2.0/fuseki-server'
source=tools/'bsbm100.nt';before=source.stat()
def metadata():
    return {str(p.relative_to(store)):{'bytes':p.stat().st_size,'mtime_ns':p.stat().st_mtime_ns} for p in store.rglob('*') if p.is_file()}
meta_before=metadata();(out/'store-before.json').write_text(json.dumps(meta_before,indent=2))
with (out/'help.txt').open('wb') as dest:
    result=subprocess.run([str(program),'--help'],stdout=dest,stderr=subprocess.STDOUT,timeout=30)
helptext=(out/'help.txt').read_text();assert '--update' in helptext and '--loc' in helptext
port=31436
with socket.socket() as sock:sock.bind(('127.0.0.1',port))
endpoint=f'http://127.0.0.1:{port}/ds/sparql'
cmd=[str(program),'--loc='+str(store),'--port='+str(port),'/ds']
(out/'runtime.json').write_text(json.dumps({'command':cmd,'environment':{'JVM_ARGS':'-Xmx8g -XX:ActiveProcessorCount=4'},'authorization':'Kaiming c59faed7 in task25: query-only shared store under owned lock','updates_requested':False,'update_flag_supplied':False,'scope':'independent input group COUNT diagnostics, not BI outputs or speed comparison','source_input_identity':json.loads((root/'bsbm100-preparation-v1/input-identity.json').read_text()),'launcher_sha256':hashlib.sha256(program.read_bytes()).hexdigest()},indent=2))
log=(out/'server.log').open('wb');server=subprocess.Popen(cmd,stdout=log,stderr=subprocess.STDOUT,start_new_session=True,env={**os.environ,'JVM_ARGS':'-Xmx8g -XX:ActiveProcessorCount=4'})
state={'pid':server.pid,'complete':False,'candidate_bi_queries_executed':False};(out/'status.json').write_text(json.dumps(state,indent=2))
def ask(query,timeout):
    request=urllib.request.Request(endpoint,data=urllib.parse.urlencode({'query':query}).encode(),headers={'Accept':'application/sparql-results+json'})
    with urllib.request.urlopen(request,timeout=timeout) as response:return response.read()
try:
    deadline=time.monotonic()+120;attempts=[]
    while True:
        if server.poll() is not None:raise RuntimeError('Jena exited before readiness; inspect log, do not clear shared lock file')
        try:ready=ask('ASK {}',3);assert json.loads(ready)['boolean'];break
        except (OSError,TimeoutError) as exc:
            attempts.append(type(exc).__name__+': '+str(exc))
            if time.monotonic()>deadline:raise
            time.sleep(.25)
    (out/'ready.json').write_bytes(ready);(out/'readiness-attempts.json').write_text(json.dumps(attempts,indent=2))
    query='SELECT (COUNT(*) AS ?count) WHERE { ?s ?p ?o }'
    (out/'count.rq').write_text(query);start=time.monotonic();raw=ask(query,180);(out/'count.json').write_bytes(raw)
    result=json.loads(raw);rows=result['results']['bindings'];assert result['head']['vars']==['count'] and len(rows)==1
    value=rows[0]['count'];assert value['type'] in ('literal','typed-literal') and value['datatype']=='http://www.w3.org/2001/XMLSchema#integer';count=int(value['value']);assert count>0
    assert count==98861444
    import sys
    diagnostic=out.parent
    sys.path.insert(0,str(diagnostic/'collector-source'))
    import numeric_inputs
    plan=json.loads((diagnostic/'plan.json').read_text())
    destination=diagnostic/'jena';destination.mkdir()
    state['cases']={}
    for key,case in plan['cases'].items():
        assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
        from supervise_import import snapshot
        point=snapshot(root,owner)
        if point['abort']:raise RuntimeError(point['abort'])
        for name,record in case['jena'].items():
            text=(diagnostic/record['file']).read_text()
            assert hashlib.sha256(text.encode()).hexdigest()==record['sha256']
            body=ask(text,180)
            json.loads(body) # Reject malformed responses; verification runs separately.
            (destination/(key+'-'+name+'.json')).write_bytes(body)
        state['cases'][key]={'complete':True}
        (out/'status.json').write_text(json.dumps(state,indent=2))
    state.update(complete=True,reference_unique_triples=count,diagnostic_elapsed_seconds=time.monotonic()-start,nt_import_count_comparison_pass=True)
except Exception as exc:
    state['error']=type(exc).__name__+': '+str(exc)
finally:
    if server.poll() is None:
        os.killpg(server.pid,signal.SIGTERM)
        try:server.wait(timeout=10)
        except subprocess.TimeoutExpired:os.killpg(server.pid,signal.SIGKILL);server.wait()
    log.close();state['server_exit']=server.returncode
    after=source.stat();state['nt_stat_unchanged']=(before.st_ino,before.st_size,before.st_mtime_ns)==(after.st_ino,after.st_size,after.st_mtime_ns)
    meta_after=metadata();(out/'store-after.json').write_text(json.dumps(meta_after,indent=2));state['store_metadata_changed']=[name for name in set(meta_before)|set(meta_after) if meta_before.get(name)!=meta_after.get(name)]
    unexpected=[name for name in state['store_metadata_changed'] if name not in ('tdb.lock','Data-0001/tdb.lock')]
    state['only_expected_lock_metadata_changed']=not unexpected
    if unexpected or not state['nt_stat_unchanged']:
        state['complete']=False;state['postcheck_error']='Source or shared store data metadata changed'
    lock=Path('/tmp/rdf-bench.lock')
    if (lock/'owner').read_text().strip()==owner:(lock/'owner').unlink();lock.rmdir();state['own_lock_released']=True
    (out/'status.json').write_text(json.dumps(state,indent=2))
print(json.dumps(state),flush=True)
if not state['complete']:raise SystemExit(1)
