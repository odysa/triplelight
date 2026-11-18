"""Read-only native HTTP qualification; elapsed observations are not a comparison."""
import hashlib,json,os,signal,socket,subprocess,sys,time
from pathlib import Path

stage=Path(__file__).resolve().parent; root=stage.parent
owner='Jianlin-Su task24 string-function qualification'
lock=Path('/tmp/rdf-bench.lock')
binary=stage/'rdfdb-ef7a778'
store=Path('<agent-workspace>/rdfdb/bench/tools/rdfdb-disk-bsbm100-main')
ref=root/'bsbm100-reference-v1'

def sha(path):
    h=hashlib.sha256()
    with path.open('rb') as f:
        while b:=f.read(8*1024*1024): h.update(b)
    return h.hexdigest()

def identity():
    return {str(p.relative_to(store)):{'size':p.stat().st_size,'mtime_ns':p.stat().st_mtime_ns,'sha256':sha(p)}
            for p in sorted(store.rglob('*')) if p.is_file()}

state={'comparative_timing_claim':False,'complete':False,'started_unix':time.time()}
process=None
try:
    assert (lock/'owner').read_text().strip()==owner
    assert sha(binary)==(stage/'binary-sha256.txt').read_text().strip()
    assert (store/'MANIFEST').is_file()
    before=identity();(stage/'store-before.json').write_text(json.dumps(before,indent=2))
    with socket.socket() as s: s.bind(('127.0.0.1',31439))
    command=[str(binary),'serve',str(store),'31439','--disk','--cache-mb=10240']
    evidence={'system':'rdfdb','endpoint':'http://127.0.0.1:31439/sparql',
      'source_or_image':'7733eb4fbd0acb99832eea184ae340c96bea9a95',
      'binary_sha256':sha(binary),'dataset_sha256':'6e4ef939f05b15677802d17257096de9da32a452fe33dd40c07d0b0537ee430d',
      'threads':4,'memory_configuration':{'index_dictionary_cache_mb':10240,'hard_process_limit':None},
      'cache_profile':'normal shared cache; statistics readiness required; no result cache override',
      'mode':'native untimed output qualification','command':command,'comparative_timing_claim':False}
    evidence.update(system='rdfdb',endpoint='http://127.0.0.1:31439/sparql',source_or_image='ef7a77814d23879a7bdf174ffe567b81a402ba9f; binary SHA256 '+sha(binary))
    (stage/'runtime-evidence.json').write_text(json.dumps(evidence,indent=2))
    env={**os.environ,'RAYON_NUM_THREADS':'4','RDFDB_HOST':'127.0.0.1'}
    with (stage/'server.log').open('wb') as log:
        process=subprocess.Popen(command,env=env,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
    (stage/'process.json').write_text(json.dumps({'pid':process.pid,'command':command}))
    deadline=time.monotonic()+600
    while True:
        if process.poll() is not None:raise RuntimeError('Server exited before statistics readiness')
        if 'statistics ready after' in (stage/'server.log').read_text(errors='replace'):break
        if time.monotonic()>deadline:raise TimeoutError('Statistics readiness')
        time.sleep(.2)
    state['statistics_ready_unix']=time.time()
    subprocess.run([sys.executable,str(stage/'string_http.py')],check=True)
    args=[sys.executable,str(stage/'bench-source/http_measure_value.py'),
      '--workload',str(root/'bsbm100-preparation-v1/workload'),'--reference',str(ref/'reference'),
      '--out',str(stage/'queries'),'--runtime-evidence',str(stage/'runtime-evidence.json'),
      '--numeric-inputs',str(root/'numeric-freeze-100m-v1'),
      '--numeric-freeze-sha256','63e127b60ac8de510a172748108639f0834e17bc3772e1d16ce226404aa58cca',
      '--count-policy-freeze',str(ref/'count-policy-freeze.json'),
      '--count-policy-sha256','5049fb75210fc6fc78ebf1fb2553b88baef197ae73495b70f110a203a70a4663',
      '--value-policy-freeze',str(ref/'value-policy-freeze.json'),
      '--value-policy-sha256','7b3f640bbab516db80ceba87549b9670802a93c5ff77181bdda66c0855fc3dfb',
      '--lock-owner',owner,'--timeout','180']
    guarded=[sys.executable,str(stage/'bench-source/supervise_import.py'),'--root',str(root),
      '--receipt',str(stage/'supervisor.json'),'--owner',owner,'--',*args]
    (stage/'command.json').write_text(json.dumps(guarded,indent=2))
    with (stage/'client.log').open('wb') as log:
        result=subprocess.run(guarded,stdout=log,stderr=subprocess.STDOUT)
    state['client_exit']=result.returncode
    if result.returncode:raise RuntimeError('Client failed; retain partial qualification')
    report=json.loads((stage/'queries/measurements.json').read_text())
    state.update(complete=report['complete'],queries=len(report['queries']),
      strict_pass=sum(q['status']=='pass' for q in report['queries'].values()),
      count_pass=sum(q.get('count_equivalence',{}).get('status')=='pass' for q in report['queries'].values()),
      value_pass=sum(q.get('value_compatibility',{}).get('status')=='pass' for q in report['queries'].values()))
except BaseException as exc:state['error']=type(exc).__name__+': '+str(exc)
finally:
    if process is not None:
        if process.poll() is None:
            os.killpg(process.pid,signal.SIGTERM)
            try: process.wait(timeout=15)
            except subprocess.TimeoutExpired:os.killpg(process.pid,signal.SIGKILL);process.wait()
        state['server_exit']=process.returncode
    try:
        after=identity();(stage/'store-after.json').write_text(json.dumps(after,indent=2))
        state['store_unchanged']=before==after
    except Exception as exc:state['store_postcheck_error']=str(exc)
    state['ended_unix']=time.time();(stage/'status.json').write_text(json.dumps(state,indent=2))
    if (lock/'owner').exists() and (lock/'owner').read_text().strip()==owner:
        (lock/'owner').unlink();lock.rmdir()
sys.exit(0 if state['complete'] and state.get('store_unchanged') else 1)
