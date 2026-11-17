from pathlib import Path
import subprocess,socket,time,os,signal,json,urllib.request,urllib.parse
root=Path(__file__).resolve().parent;out=root/'workload-check-smoke'
python='<agent-workspace>/scratch/analytical25-venv/bin/python'
owner='Jianlin-Su task25 DuckDB 10M baseline import'
assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
source=root/'fixture-float/data.nt'
workload=out/'workload';manifest=json.loads((workload/'manifest.json').read_text())
port=31425
with socket.socket() as s:s.bind(('127.0.0.1',port))
endpoint=f'http://127.0.0.1:{port}/ds/sparql'
command=['<agent-workspace>/rdfdb/bench/tools/apache-jena-fuseki-6.2.0/fuseki-server','--file='+str(source),'--port='+str(port),'/ds']
log=(out/'jena-server.log').open('wb')
process=subprocess.Popen(command,env={**os.environ,'JVM_ARGS':'-Xmx1g'},stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
(out/'jena-process.json').write_text(json.dumps({'pid':process.pid,'command':command,'jvm_args':'-Xmx1g'}))
common=[python,str(out/'source/analytical/workload_check.py'),'--workload',str(workload),'--dataset-sha256',manifest['source_data_sha256'],'--lock-owner',owner,'--timeout','30']
try:
    for i in range(100):
        if process.poll() is not None:raise RuntimeError('Jena failed')
        try:
            req=urllib.request.Request(endpoint,data=urllib.parse.urlencode({'query':'ASK {}'}).encode(),headers={'Accept':'application/sparql-results+json'})
            with urllib.request.urlopen(req,timeout=1) as response:json.load(response)
            break
        except Exception:time.sleep(.1)
    else:raise TimeoutError('Jena readiness')
    with (out/'jena.log').open('wb') as dest:
        subprocess.run(common+['--system','jena','--endpoint',endpoint,'--out',str(out/'jena'),'--runtime-source','Jena 6.2.0 fresh --file fixture'],stdout=dest,stderr=subprocess.STDOUT,check=True)
finally:
    if process.poll() is None:
        os.killpg(process.pid,signal.SIGTERM)
        try:process.wait(timeout=10)
        except subprocess.TimeoutExpired:os.killpg(process.pid,signal.SIGKILL);process.wait()
    log.close()
for system,extra in [('duckdb',['--database',str(root/'duck-metrics-pilot-v2/duck.db'),'--import-report',str(root/'duck-metrics-pilot-v2/report.json'),'--runtime-source','DuckDB2.0.0']) ,('rdfdb',['--database',str(out/'rdfdb-store'),'--source',str(source),'--runtime-source','d2618c6 isolated rebuilt Python binding'])]:
    with (out/(system+'.log')).open('wb') as dest:
        env={**os.environ,'RAYON_NUM_THREADS':'4'};env.pop('PYTHONPATH',None)
        subprocess.run(common+['--system',system,'--reference',str(out/'jena'),'--out',str(out/system),*extra],env=env,stdout=dest,stderr=subprocess.STDOUT,check=True)
for system in ['jena','duckdb','rdfdb']:
    matrix=json.loads((out/system/'matrix.json').read_text())
    print(system,matrix['complete'],matrix['all_requested_pass'],flush=True)
print('SMOKE COMPLETE',flush=True)
