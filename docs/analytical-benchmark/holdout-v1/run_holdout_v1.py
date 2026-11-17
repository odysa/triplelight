"""Serial host correctness cohort under the immutable numeric freeze; no speed score."""
from pathlib import Path
import argparse,subprocess,socket,time,os,signal,json,urllib.request,urllib.parse,hashlib
p=argparse.ArgumentParser();p.add_argument('--variants',type=Path,required=True);args=p.parse_args()
root=Path(__file__).resolve().parent;out=root/'bsbm10-holdout-v1'
owner='Jianlin-Su task25 BI holdout frozen v1'
assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
python='<agent-workspace>/scratch/analytical25-venv/bin/python'
inputs=root/'numeric-diagnostics-10m';freeze_hash='a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244'
assert hashlib.sha256((inputs/'freeze.json').read_bytes()).hexdigest()==freeze_hash
assert not out.exists();out.mkdir()
from shutil import copytree
copytree(Path('<agent-workspace>/rdfdb/bench/analytical'),out/'source',ignore=lambda path,names:[n for n in names if n=='__pycache__'])
workload=out/'source/workloads/bsbm10';manifest=json.loads((workload/'manifest.json').read_text())
variants=json.loads(args.variants.read_text());(out/'variants.json').write_text(json.dumps(variants,indent=2))
(out/'cohort.json').write_text(json.dumps({'freeze_sha256':freeze_hash,'all_manifest_queries':60,'exploratory_mix_indices':[0],
    'unseen_confirmatory_mix_indices':[1,2,3],'mode':'host correctness, not competitor timing','variants_source':str(args.variants)},indent=2))
source=Path('<agent-workspace>/rdfdb/bench/tools/bsbm10.nt')
helper=out/'source/workload_check.py'
common=[python,str(helper),'--workload',str(workload),'--dataset-sha256',manifest['source_data_sha256'],'--lock-owner',owner,
        '--numeric-inputs',str(inputs),'--numeric-freeze-sha256',freeze_hash,'--timeout','120']
port=31425
with socket.socket() as s:s.bind(('127.0.0.1',port))
endpoint=f'http://127.0.0.1:{port}/ds/sparql';status={}
command=['<agent-workspace>/rdfdb/bench/tools/apache-jena-fuseki-6.2.0/fuseki-server','--file='+str(source),'--port='+str(port),'/ds']
log=(out/'jena-server.log').open('wb')
server=subprocess.Popen(command,env={**os.environ,'JVM_ARGS':'-Xmx12g'},stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
(out/'jena-process.json').write_text(json.dumps({'pid':server.pid,'command':command,'heap_setting':'12g'}))
try:
    deadline=time.monotonic()+180
    while time.monotonic()<deadline:
        if server.poll() is not None:raise RuntimeError('Jena failed before readiness')
        try:
            request=urllib.request.Request(endpoint,data=urllib.parse.urlencode({'query':'ASK {}'}).encode(),headers={'Accept':'application/sparql-results+json'})
            with urllib.request.urlopen(request,timeout=2) as response:json.load(response)
            break
        except Exception:time.sleep(.25)
    else:raise TimeoutError('Jena readiness')
    with (out/'jena.log').open('wb') as dest:
        result=subprocess.run(common+['--system','jena','--endpoint',endpoint,'--out',str(out/'jena'),'--runtime-source','Jena6.2.0 fresh pinned10M graph'],stdout=dest,stderr=subprocess.STDOUT,timeout=900)
    status['jena']={'exit_code':result.returncode}
finally:
    if server.poll() is None:
        os.killpg(server.pid,signal.SIGTERM)
        try:server.wait(timeout=10)
        except subprocess.TimeoutExpired:os.killpg(server.pid,signal.SIGKILL);server.wait()
    log.close()
print('Jena references finished and process exited',flush=True)
(out/'status.json').write_text(json.dumps(status,indent=2))
jobs=[('duckdb','duckdb',['--database',str(root/'bsbm10-duckdb/duck.db'),'--import-report',str(root/'bsbm10-duckdb/import.json'),'--runtime-source','DuckDB2.0.0 plain triples'],{})]
for variant in variants:
    env={'PYTHONPATH':variant['pythonpath'],'RAYON_NUM_THREADS':'4'}
    if variant['legacy_group']:env['RDFDB_LEGACY_GROUP']='1'
    jobs.append((variant['name'],'rdfdb',['--database',str(root/'bsbm10-parameter-check/rdfdb-store'),
        '--source',str(source),'--runtime-source',variant['source_commit']+'; '+variant['name']],env))
for name,system,extra,settings in jobs:
    assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
    env={**os.environ};env.pop('PYTHONPATH',None);env.pop('RDFDB_LEGACY_GROUP',None);env.update(settings)
    if system=='rdfdb':
        variant=next(v for v in variants if v['name']==name)
        check='import rdfdb, pathlib, hashlib, json; p=pathlib.Path(rdfdb.__file__).resolve(); s=list(p.parent.glob("*.so")); print(json.dumps({"package":str(p),"extension_sha256":hashlib.sha256(s[0].read_bytes()).hexdigest()}))'
        observed=json.loads(subprocess.check_output([python,'-c',check],env=env,text=True))
        if observed['package']!=variant['package'] or observed['extension_sha256']!=variant['extension_sha256']:
            raise RuntimeError('Runtime binding differs: '+name)
        (out/(name+'-runtime.json')).write_text(json.dumps(observed,indent=2))
    with (out/(name+'.log')).open('wb') as dest:
        try:
            result=subprocess.run(common+['--system',system,'--reference',str(out/'jena'),'--out',str(out/name),*extra],env=env,stdout=dest,stderr=subprocess.STDOUT,timeout=900)
            status[name]={'exit_code':result.returncode}
        except subprocess.TimeoutExpired:status[name]={'error':'900s phase timeout; own worker killed and waited'}
    (out/'status.json').write_text(json.dumps(status,indent=2))
    matrix=out/name/'matrix.json'
    if matrix.exists():
        value=json.loads(matrix.read_text());print(name,value['complete'],value.get('all_requested_pass'),[(q,v.get('error')) for q,v in value['queries'].items() if v['status']!='pass'],flush=True)
    else:print(name,status[name],flush=True)
print('HOLDOUT CHECK FINISHED; all failures retained; no performance score emitted',flush=True)
