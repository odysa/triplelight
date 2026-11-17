from pathlib import Path
import argparse,subprocess,socket,time,os,signal,json,urllib.request,urllib.parse
p=argparse.ArgumentParser();p.add_argument('--size',choices=['smoke','10m'],required=True);args=p.parse_args()
root=Path(__file__).resolve().parent;out=root/('numeric-diagnostics-'+args.size)
python='<agent-workspace>/scratch/analytical25-venv/bin/python'
owner='Jianlin-Su task25 numeric validator and input diagnostics'
assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
workload=root/'workload-check-smoke/workload' if args.size=='smoke' else root/'bsbm10-parameter-check/workload'
source=root/'fixture-float/data.nt' if args.size=='smoke' else Path('<agent-workspace>/rdfdb/bench/tools/bsbm10.nt')
database=root/'duck-metrics-pilot-v2/duck.db' if args.size=='smoke' else root/'bsbm10-duckdb/duck.db'
report=root/'duck-metrics-pilot-v2/report.json' if args.size=='smoke' else root/'bsbm10-duckdb/import.json'
helper=Path('<agent-workspace>/rdfdb/bench/analytical/numeric_inputs.py')
subprocess.run([python,str(helper),'--phase','make','--workload',str(workload),'--out',str(out)],check=True)
port=31425
with socket.socket() as s:s.bind(('127.0.0.1',port))
endpoint=f'http://127.0.0.1:{port}/ds/sparql'
command=['<agent-workspace>/rdfdb/bench/tools/apache-jena-fuseki-6.2.0/fuseki-server','--file='+str(source),'--port='+str(port),'/ds']
log=(out/'jena-server.log').open('wb')
process=subprocess.Popen(command,env={**os.environ,'JVM_ARGS':'-Xmx1g' if args.size=='smoke' else '-Xmx12g'},stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
(out/'jena-process.json').write_text(json.dumps({'pid':process.pid,'command':command}))
try:
    deadline=time.monotonic()+180
    while time.monotonic()<deadline:
        if process.poll() is not None:raise RuntimeError('Jena failed')
        try:
            req=urllib.request.Request(endpoint,data=urllib.parse.urlencode({'query':'ASK {}'}).encode(),headers={'Accept':'application/sparql-results+json'})
            with urllib.request.urlopen(req,timeout=2) as response:json.load(response)
            break
        except Exception:time.sleep(.1)
    else:raise TimeoutError('Jena readiness')
    with (out/'jena.log').open('wb') as dest:
        subprocess.run([python,str(helper),'--phase','jena','--out',str(out),'--endpoint',endpoint,'--lock-owner',owner],stdout=dest,stderr=subprocess.STDOUT,check=True,timeout=900)
finally:
    if process.poll() is None:
        os.killpg(process.pid,signal.SIGTERM)
        try:process.wait(timeout=10)
        except subprocess.TimeoutExpired:os.killpg(process.pid,signal.SIGKILL);process.wait()
    log.close()
print('Jena counts complete; service exited',flush=True)
with (out/'sql.log').open('wb') as dest:
    subprocess.run([python,str(helper),'--phase','sql','--out',str(out),'--database',str(database),'--import-report',str(report),'--lock-owner',owner],stdout=dest,stderr=subprocess.STDOUT,check=True,timeout=900)
subprocess.run([python,str(helper),'--phase','verify','--out',str(out)],check=True)
print(args.size,'DIAGNOSTIC COUNTS VERIFIED',flush=True)
