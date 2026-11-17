from pathlib import Path
import subprocess,socket,time,os,signal,json,urllib.request,urllib.parse,hashlib
root=Path(__file__).resolve().parent;out=root/'bsbm10-parameter-check'
python='<agent-workspace>/scratch/analytical25-venv/bin/python'
owner='Jianlin-Su task25 DuckDB 10M baseline import'
assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
source=Path('<agent-workspace>/rdfdb/bench/tools/bsbm10.nt')
workload=out/'workload';manifest=json.loads((workload/'manifest.json').read_text())
port=31425
with socket.socket() as s:s.bind(('127.0.0.1',port))
endpoint=f'http://127.0.0.1:{port}/ds/sparql'
command=['<agent-workspace>/rdfdb/bench/tools/apache-jena-fuseki-6.2.0/fuseki-server','--file='+str(source),'--port='+str(port),'/ds']
log=(out/'jena-server.log').open('wb')
process=subprocess.Popen(command,env={**os.environ,'JVM_ARGS':'-Xmx12g'},stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
(out/'jena-process.json').write_text(json.dumps({'pid':process.pid,'command':command,'jvm_args':'-Xmx12g'}))
common=[python,str(out/'source/analytical/workload_check.py'),'--workload',str(workload),'--dataset-sha256',manifest['source_data_sha256'],'--lock-owner',owner,'--timeout','120','--limit','15']
status={}
try:
    deadline=time.monotonic()+180
    while time.monotonic()<deadline:
        if process.poll() is not None:raise RuntimeError('Jena failed before readiness')
        try:
            req=urllib.request.Request(endpoint,data=urllib.parse.urlencode({'query':'ASK {}'}).encode(),headers={'Accept':'application/sparql-results+json'})
            with urllib.request.urlopen(req,timeout=2) as response:json.load(response)
            break
        except Exception:time.sleep(.25)
    else:raise TimeoutError('Jena readiness')
    print('Jena ready; first 15 parameter references starting',flush=True)
    with (out/'jena.log').open('wb') as dest:
        result=subprocess.run(common+['--system','jena','--endpoint',endpoint,'--out',str(out/'jena'),'--runtime-source','Jena6.2.0 fresh --file pinned10M'],stdout=dest,stderr=subprocess.STDOUT,timeout=900)
    status['jena']={'exit_code':result.returncode}
finally:
    if process.poll() is None:
        os.killpg(process.pid,signal.SIGTERM)
        try:process.wait(timeout=10)
        except subprocess.TimeoutExpired:os.killpg(process.pid,signal.SIGKILL);process.wait()
    log.close()
print('Reference process exited; candidates run serially',flush=True)
(out/'status.json').write_text(json.dumps(status,indent=2))
for name,system,extra in [
 ('duckdb','duckdb',['--database',str(root/'bsbm10-duckdb/duck.db'),'--import-report',str(root/'bsbm10-duckdb/import.json'),'--runtime-source','DuckDB2.0.0 plain triples']),
 ('rdfdb','rdfdb',['--database',str(out/'rdfdb-store'),'--source',str(source),'--runtime-source','cb827db96c62ee440539eac31e5d73c771b2ceb1 isolated Python; new GROUP path']),
 ('rdfdb-legacy','rdfdb',['--database',str(out/'rdfdb-store'),'--source',str(source),'--runtime-source','cb827db96c62ee440539eac31e5d73c771b2ceb1 same Python; RDFDB_LEGACY_GROUP=1'])]:
    assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
    env={**os.environ,'RAYON_NUM_THREADS':'4'};env.pop('PYTHONPATH',None);env.pop('RDFDB_LEGACY_GROUP',None)
    if name=='rdfdb-legacy':env['RDFDB_LEGACY_GROUP']='1'
    with (out/(name+'.log')).open('wb') as dest:
        try:
            result=subprocess.run(common+['--system',system,'--reference',str(out/'jena'),'--out',str(out/name),*extra],env=env,stdout=dest,stderr=subprocess.STDOUT,timeout=900)
            status[name]={'exit_code':result.returncode}
        except subprocess.TimeoutExpired:
            status[name]={'error':'phase timeout at900seconds; own worker killed and waited by subprocess.run'}
    (out/'status.json').write_text(json.dumps(status,indent=2))
    matrix=out/name/'matrix.json'
    if matrix.exists():
        value=json.loads(matrix.read_text());print(name,value['complete'],value.get('all_requested_pass'),[(k,v.get('error')) for k,v in value['queries'].items() if v['status']!='pass'],flush=True)
    else:print(name,status[name],flush=True)
    if name=='rdfdb' and status[name].get('exit_code')!=0:
        print('Skipping legacy after incomplete first RDF worker',flush=True);break
print('REAL PARAMETER CHECK FINISHED',flush=True)
