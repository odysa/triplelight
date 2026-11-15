#!/usr/bin/env python3
"""Independent aggregate checks, under the shared benchmark lock. Starts/stops own Jena server."""
import gzip,sys,urllib.request,urllib.parse,json,time,subprocess,os,signal,socket
from pathlib import Path
family,tools,candidate=sys.argv[1:]
root=Path(__file__).resolve().parent
tools=Path(tools);port=3094
with socket.socket() as probe:
    assert probe.connect_ex(('127.0.0.1',port))!=0,'port busy: refusing stale server'
log=root/'results'/('jena-'+family+'.log')
cmd=[str(tools/'apache-jena-fuseki-6.2.0/fuseki-server'),'--loc='+str(tools/('tdb2-'+('lubm100' if family=='lubm' else 'watdiv100'))),'--port='+str(port),'/ds']
env=dict(os.environ,JVM_ARGS='-Xmx4g')
def request(query):
    data=urllib.parse.urlencode({'query':query}).encode()
    req=urllib.request.Request('http://127.0.0.1:'+str(port)+'/ds/sparql',data=data,headers={'Accept':'application/sparql-results+json'})
    return json.load(urllib.request.urlopen(req,timeout=60))
def normalize(value):
    return sorted(tuple(sorted((var,term.get('type'),term.get('value'),term.get('datatype'),term.get('xml:lang')) for var,term in row.items())) for row in value['results']['bindings'])
with log.open('w') as handle:
    process=subprocess.Popen(cmd,stdout=handle,stderr=subprocess.STDOUT,env=env,start_new_session=True)
    try:
        for _ in range(100):
            if process.poll() is not None:raise RuntimeError('Jena exited')
            try:request('ASK {?s ?p ?o}');break
            except Exception:time.sleep(.2)
        else:raise RuntimeError('Jena did not become ready')
        names=['group_advisors','group_type','distinct_q6','distinct_q9','all_q2','all_q9'] if family=='lubm' else ['group_age','group_gender','distinct_C3','distinct_L2','all_C3']
        checks=[]
        for name in names:
            value=request((root/'queries'/family/(name+'.rq')).read_text())
            expected_path=Path(candidate+'.answers')/(name+'.json')
            expected=json.loads(expected_path.read_text() if expected_path.exists() else gzip.decompress(expected_path.with_suffix('.json.gz').read_bytes()))
            assert normalize(value)==normalize(expected),(family,name,'Jena disagreement')
            (root/'results'/('jena-'+family+'-'+name+'.json')).write_text(json.dumps(value)+'\n')
            checks.append({'query':name,'rows':len(value['results']['bindings']),'equal':True})
        (root/'results'/('jena-'+family+'-checks.json')).write_text(json.dumps({'command':cmd,'checks':checks},indent=2)+'\n')
        print(family,len(checks),'Jena checks passed')
    finally:
        if process.poll() is None:
            os.killpg(process.pid,signal.SIGTERM)
            try:process.wait(timeout=10)
            except subprocess.TimeoutExpired:os.killpg(process.pid,signal.SIGKILL);process.wait()
