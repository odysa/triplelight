"""Pinned read-only QLever output qualification, no comparative timing claim."""
import hashlib,json,subprocess,sys,time,urllib.request,urllib.parse
from pathlib import Path
stage=Path(__file__).resolve().parent;root=stage.parent
owner='Jianlin-Su task25 pinned QLever100M build and qualification'
lock=Path('/tmp/rdf-bench.lock');build=root/'qlever100-build-v1'
image='adfreiburg/qlever@sha256:d7ecda47b4d0132c190b4a73279761fd1c614f24cba4e894ca1ba8117cef6727'
container='rdf-jianlin-qlever100-qualification-v1'
endpoint='http://127.0.0.1:31438/'
def sha(path):
    h=hashlib.sha256()
    with path.open('rb') as f:
        while b:=f.read(8*1024*1024):h.update(b)
    return h.hexdigest()
def index_identity():
    return {str(p.relative_to(build/'index')):{'bytes':p.stat().st_size,'sha256':sha(p)}
      for p in sorted((build/'index').rglob('*')) if p.is_file()}
def ask(fields):
    request=urllib.request.Request(endpoint,data=urllib.parse.urlencode(fields).encode(),headers={'Content-Type':'application/x-www-form-urlencoded','Accept':'application/sparql-results+json'})
    with urllib.request.urlopen(request,timeout=30) as response:return json.load(response)
state={'complete':False,'comparative_timing_claim':False,'started_unix':time.time()}
try:
    assert (lock/'owner').read_text().strip()==owner
    b=json.loads((build/'status.json').read_text());assert b['complete']
    before=index_identity();assert before==b['index_files']
    (stage/'index-before.json').write_text(json.dumps(before,indent=2))
    (stage/'tmp').mkdir(exist_ok=True)
    command=['docker','run','-d','--name',container,'--cpus=4','--memory=12g','--memory-swap=12g',
      '-p','127.0.0.1:31438:31438','--mount',f'type=bind,source={build}/index,target=/index,readonly',
      '--mount',f'type=bind,source={stage}/tmp,target=/tmp','--workdir','/index',
      '--entrypoint','/qlever/qlever-server',image,'-i','bsbm100','-p','31438','-m','11GB',
      '-c','1MB','-e','1MB','-E','1MB','-s','180s',
      '--set-runtime-parameter','max-num-threads-per-query=4','--no-metrics-log','--no-resource-usage-log']
    (stage/'server-command.json').write_text(json.dumps(command,indent=2))
    identifier=subprocess.check_output(command,text=True).strip();(stage/'container-id').write_text(identifier)
    deadline=time.monotonic()+180
    while True:
        try:ask({'query':'ASK {}'});break
        except Exception:
            if time.monotonic()>deadline:raise
            time.sleep(.3)
    count=ask({'query':'SELECT (COUNT(*) AS ?n) WHERE {?s ?p ?o}'})
    (stage/'count.json').write_text(json.dumps(count,indent=2))
    assert int(count['results']['bindings'][0]['n']['value'])==98861444
    settings=ask({'cmd':'get-settings'});(stage/'settings.json').write_text(json.dumps(settings,indent=2))
    assert settings['memory-for-queries']=='11 GB' and settings['max-num-threads-per-query']=='4'
    assert settings['disable-caching']=='false' and settings['group-by-hash-map-enabled']=='false'
    counters=ask({'cmd':'clear-cache'});(stage/'initial-clear.json').write_text(json.dumps(counters,indent=2))
    assert len(counters)==5 and all(v==0 for v in counters.values())
    evidence={'system':'qlever','endpoint':endpoint,'source_or_image':image,
      'dataset_sha256':'6e4ef939f05b15677802d17257096de9da32a452fe33dd40c07d0b0537ee430d',
      'threads':4,'memory_configuration':{'container_bytes':12*1024**3,'query_memory':'11 GB'},
      'cache_profile':'clear-between-requests','cache_control_verified':True,
      'cache_control_evidence':'Pinned image live settings.json and initial-clear.json; five counters zero',
      'before_each_request':{'endpoint':endpoint,'fields':{'cmd':'clear-cache'},'expected_json':counters},
      'mode':'native client to VM container untimed output qualification; not a matched comparison'}
    (stage/'runtime-evidence.json').write_text(json.dumps(evidence,indent=2))
    ref=root/'bsbm100-reference-v1'
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
    (stage/'client-command.json').write_text(json.dumps(guarded,indent=2))
    with (stage/'client.log').open('wb') as log:result=subprocess.run(guarded,stdout=log,stderr=subprocess.STDOUT)
    state['client_exit']=result.returncode
    if result.returncode:raise RuntimeError('Qualification client failed')
    report=json.loads((stage/'queries/measurements.json').read_text())
    state.update(complete=report['complete'],queries=len(report['queries']),
      primary_pass=sum(q['status']=='pass' for q in report['queries'].values()),
      count_pass=sum(q.get('count_equivalence',{}).get('status')=='pass' for q in report['queries'].values()),
      value_pass=sum(q.get('value_compatibility',{}).get('status')=='pass' for q in report['queries'].values()))
except BaseException as exc:state['error']=type(exc).__name__+': '+str(exc)
finally:
    inspect=subprocess.run(['docker','inspect',container,'--format','{{json .State}}'],capture_output=True,text=True)
    if inspect.returncode==0:
        state['container_state_before_cleanup']=json.loads(inspect.stdout)
        logs=subprocess.run(['docker','logs',container],capture_output=True)
        (stage/'server.log').write_bytes(logs.stdout+logs.stderr)
        if state['container_state_before_cleanup'].get('Running'):
            subprocess.run(['docker','stop','--time','15',container],capture_output=True,check=True)
        subprocess.run(['docker','rm',container],capture_output=True,check=True)
    try:
        after=index_identity();(stage/'index-after.json').write_text(json.dumps(after,indent=2))
        state['index_unchanged']=before==after
    except Exception as exc:state['postcheck_error']=str(exc)
    state['ended_unix']=time.time();(stage/'status.json').write_text(json.dumps(state,indent=2))
    if (lock/'owner').exists() and (lock/'owner').read_text().strip()==owner:
        (lock/'owner').unlink();lock.rmdir()
sys.exit(0 if state['complete'] and state.get('index_unchanged') else 1)
