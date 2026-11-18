import json,os,signal,socket,subprocess,time,urllib.request,urllib.parse
from pathlib import Path
p=Path(__file__).resolve().parent
data=p/'sequence-fixture.nt'
data.write_text('<urn:s> <urn:p> <urn:m1> .\n<urn:s> <urn:p> <urn:m2> .\n<urn:m1> <urn:q> <urn:o> .\n<urn:m2> <urn:q> <urn:o> .\n')
with socket.socket() as s:s.bind(('127.0.0.1',31437))
cmd=[str(p/'rdfdb-a202b787'),'serve',str(data),'31437','--no-stats']
with (p/'sequence-server.log').open('wb') as log:
    process=subprocess.Popen(cmd,env={**os.environ,'RDFDB_HOST':'127.0.0.1','RAYON_NUM_THREADS':'4'},stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
(p/'sequence-process.json').write_text(json.dumps({'pid':process.pid,'command':cmd}))
def ask(query):
    request=urllib.request.Request('http://127.0.0.1:31437/sparql',data=urllib.parse.urlencode({'query':query}).encode(),headers={'Accept':'application/sparql-results+json'})
    with urllib.request.urlopen(request,timeout=5) as response:return json.load(response)
try:
    deadline=time.monotonic()+20
    while True:
        try:ask('ASK {}');break
        except Exception:
            if process.poll() is not None or time.monotonic()>deadline:raise
            time.sleep(.1)
    queries={'sequence':'SELECT ?s ?o WHERE {?s <urn:p>/<urn:q> ?o}',
      'join':'SELECT ?s ?o WHERE {?s <urn:p> ?m . ?m <urn:q> ?o}'}
    results={name:ask(query) for name,query in queries.items()}
    expected=[{'s':{'type':'uri','value':'urn:s'},'o':{'type':'uri','value':'urn:o'}}]*2
    for name,result in results.items():assert result['results']['bindings']==expected,(name,result)
    (p/'sequence-control.json').write_text(json.dumps({'queries':queries,'results':results,'expected':expected,'pass':True},indent=2))
finally:
    if process.poll() is None:
        os.killpg(process.pid,signal.SIGTERM)
        try:process.wait(timeout=10)
        except subprocess.TimeoutExpired:os.killpg(process.pid,signal.SIGKILL);process.wait()
