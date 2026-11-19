from pathlib import Path
import subprocess,os,json,time,signal,hashlib,socket,urllib.request,urllib.parse,sys
from collections import Counter
root=Path(__file__).parent; parent=root.parent
owner='Jianlin task34 stats c3a86d46 BI60 correctness'
assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
repo=Path('<agent-workspace>/rdfdb')
python=str(repo.parent/'scratch/analytical25-venv/bin/python')
binary=root/'triplelight-c3a86d46'
assert __import__('hashlib').sha256(binary.read_bytes()).hexdigest()==json.loads((root/'build-status.json').read_text())['binary_sha256']
source=json.loads((root/'source.json').read_text())
for name,digest in source['files'].items():assert hashlib.sha256((root/'source'/name).read_bytes()).hexdigest()==digest
binding={'sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'bytes':binary.stat().st_size,'commit':source['commit']}
(root/'binary.json').write_text(json.dumps(binding,indent=2))
store=parent/'bsbm10-parameter-check/rdfdb-store'
def store_identity():
    return {str(p.relative_to(store)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(store.rglob('*')) if p.is_file()}
identity=store_identity()
(root/'store-before.json').write_text(json.dumps(identity,indent=2))
port=31466
with socket.socket() as sock:sock.bind(('127.0.0.1',port))
endpoint=f'http://127.0.0.1:{port}/sparql'
command=[str(binary),'serve-store',str(store),str(port),'--disk','--cache-mb=10240']
evidence={'system':'rdfdb','endpoint':endpoint,'source_or_image':source['commit'],'dataset_sha256':'3a207c6725820036eec87d08dd414090d3548ac2839ca192414dad6e352f680e','threads':4,'memory_configuration':{'cache_mib':10240,'host_no_container_cap':True},'cache_profile':'no-answer-cache','binary':binding,'server_command':command,'store_manifest_hashes':identity,'scope':'host candidate correctness only; historical VM output comparison is not an A/B timing'}
(root/'runtime.json').write_text(json.dumps(evidence,indent=2))
log=(root/'server.log').open('wb')
server=subprocess.Popen(command,env={**os.environ,'RAYON_NUM_THREADS':'4'},stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
status={'server_pid':server.pid,'complete':False}
try:
    deadline=time.monotonic()+120
    while time.monotonic()<deadline:
        if server.poll() is not None:raise RuntimeError('server exited')
        try:
            req=urllib.request.Request(endpoint,data=urllib.parse.urlencode({'query':'ASK {}'}).encode())
            with urllib.request.urlopen(req,timeout=5) as response:value=json.load(response)
            assert value['boolean'] is True
            break
        except (OSError,TimeoutError):time.sleep(.2)
    else:raise TimeoutError('readiness')
    req=urllib.request.Request(endpoint,data=urllib.parse.urlencode({'query':'SELECT (COUNT(*) AS ?n) WHERE {?s ?p ?o}'}).encode())
    with urllib.request.urlopen(req,timeout=120) as response: counted=response.read()
    (root/'graph-count.json').write_bytes(counted)
    assert json.loads(counted)['results']['bindings'][0]['n']['value']=='10102032'
    cmd=[python,str(root/'validator-source/http_measure.py'),'--workload',str(root/'workload'),'--reference',str(parent/'bsbm10-holdout-v1/jena'),'--out',str(root/'queries'),'--runtime-evidence',str(root/'runtime.json'),'--numeric-inputs',str(parent/'numeric-diagnostics-10m'),'--numeric-freeze-sha256','a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244','--lock-owner',owner,'--timeout','120']
    (root/'command.json').write_text(json.dumps(cmd,indent=2))
    with (root/'client.log').open('wb') as dest:
        child=subprocess.Popen(cmd,stdout=dest,stderr=subprocess.STDOUT,start_new_session=True)
        try:status['client_exit']=child.wait(timeout=900)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid,signal.SIGTERM);child.wait();status['client_timeout']=True
    sys.path.insert(0,str(root/'validator-source'))
    from qualify import exact_term,ORDER_KEYS
    manifest=json.loads((root/'workload/manifest.json').read_text())
    baseline=parent/'matched-vm-10m/round1-retry1/pass1-rdfdb/queries'
    comparison={}
    def rows(doc,variables):return [tuple(exact_term(row.get(v)) for v in variables) for row in doc['results']['bindings']]
    for query in manifest['queries']:
        stem=Path(query['file']).stem;n=query['template'];entry={'template':n};comparison[query['file']]=entry
        try:
            actual=json.loads((root/'queries'/(stem+'.body')).read_bytes());ref=json.loads((baseline/(stem+'.body')).read_bytes());variables=ref['head']['vars']
            a,b=rows(actual,variables),rows(ref,variables)
            entry.update(exact_term_bag=Counter(a)==Counter(b),ordered_sequence=(a==b if n in ORDER_KEYS else None),rows=len(a))
        except Exception as exc:entry['error']=str(exc)
    (root/'baseline-comparison.json').write_text(json.dumps(comparison,indent=2))
    status['complete']=True
finally:
    if server.poll() is None:
        os.killpg(server.pid,signal.SIGTERM)
        try:server.wait(timeout=10)
        except subprocess.TimeoutExpired:os.killpg(server.pid,signal.SIGKILL);server.wait()
    log.close();status['server_exit']=server.returncode
    status['binary_posthash']=hashlib.sha256(binary.read_bytes()).hexdigest()
    status['store_identity_unchanged']=all(hashlib.sha256((store/name).read_bytes()).hexdigest()==digest for name,digest in identity.items())
    (root/'status.json').write_text(json.dumps(status,indent=2))
after=store_identity();(root/'store-after.json').write_text(json.dumps(after,indent=2));status['store_unchanged']=identity==after
for name,digest in source['files'].items():assert hashlib.sha256((root/'source'/name).read_bytes()).hexdigest()==digest
status['binary_unchanged']=hashlib.sha256(binary.read_bytes()).hexdigest()==binding['sha256']
(root/'status.json').write_text(json.dumps(status,indent=2))
assert status['client_exit']==0 and status['complete'] and status['store_unchanged'] and status['binary_unchanged'],status
assert len(comparison)==60 and all(e.get('exact_term_bag') and e.get('ordered_sequence') is not False for e in comparison.values()),comparison
print(json.dumps(status),flush=True)
