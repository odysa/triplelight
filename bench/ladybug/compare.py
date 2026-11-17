#!/usr/bin/env python3
"""Serial, checkpointed, host-side comparison. Caller must own benchmark lock."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import signal
import socket
import statistics
import subprocess
import time
import urllib.parse
import urllib.request


def signature(res):
    vs=res['head']['vars']
    def value(b):
        if b is None:return None
        x=dict(b)
        if x['type']=='typed-literal':x['type']='literal'
        if x.get('datatype')=='http://www.w3.org/2001/XMLSchema#string':x.pop('datatype')
        if 'xml:lang' in x:x['xml:lang']=x['xml:lang'].lower()
        return x
    rows=sorted(json.dumps([value(row.get(v)) for v in vs],sort_keys=True,separators=(',',':'),ensure_ascii=False) for row in res['results']['bindings'])
    return {'rows':len(rows),'sha256':hashlib.sha256('\n'.join(rows).encode()).hexdigest()}


def ask(url,q):
    request=urllib.request.Request(url,data=urllib.parse.urlencode({'query':q}).encode(),headers={'Accept':'application/sparql-results+json','Content-Type':'application/x-www-form-urlencoded'})
    start=time.perf_counter()
    with urllib.request.urlopen(request,timeout=600) as r:body=r.read()
    elapsed=(time.perf_counter()-start)*1000
    return elapsed,json.loads(body)


def stop(p):
    if p.poll() is not None:return
    os.killpg(p.pid,signal.SIGTERM)
    try:p.wait(timeout=15)
    except subprocess.TimeoutExpired:
        os.killpg(p.pid,signal.SIGKILL);p.wait()


def run(args):
    out=args.out;out.mkdir(parents=True,exist_ok=True)
    if not Path('/tmp/rdf-bench.lock/owner').exists():raise RuntimeError('No benchmark lock; caller must own it')
    compiled=json.loads(args.compiled.read_text())
    queries={q.stem:q.read_text() for q in args.queries.glob('*.rq')}
    results={'schema':'ladybug-host-v1','runs':args.runs,'threads':4,'cache_mb':10240,
             'query_boundary':'HTTP roundtrip including complete serialization and transfer; client JSON parsing/hash excluded',
             'unsupported':{q:r['unsupported'] for q,r in compiled.items() if 'unsupported' in r},'systems':{}}
    reference={}
    def checkpoint():
        target=out/'results.json';temp=target.with_suffix('.tmp');temp.write_text(json.dumps(results,indent=2));temp.replace(target)
    env={**os.environ,'RAYON_NUM_THREADS':'4'}
    tools=args.tools.resolve();native=Path(__file__).with_name('native.py').resolve()
    servers=[
        ('jena',[str(tools/'apache-jena-fuseki-6.2.0/fuseki-server'),'--loc='+str(tools/('tdb2-'+args.dataset)),'--port=7982','/ds'],'http://127.0.0.1:7982/ds/sparql',{'JVM_ARGS':'-Xmx10g'}),
        ('ladybug',[str(args.python),str(native),'serve',str(args.database),str(args.compiled),'--port','7981','--memory-mb','10240','--threads','4'],'http://127.0.0.1:7981/',{}),
        ('rdfdb-memory',[str(args.rdfdb),'serve',str(tools/(args.dataset+'.nt')),'7983','--parallel-load=4'],'http://127.0.0.1:7983/sparql',{}),
        ('rdfdb-disk',[str(args.rdfdb),'serve',str(tools/('rdfdb-disk-'+args.dataset)),'7983','--disk','--cache-mb=10240'],'http://127.0.0.1:7983/sparql',{})]
    for name,cmd,url,extraenv in servers:
        port=urllib.parse.urlsplit(url).port
        with socket.socket() as probe_socket:
            if probe_socket.connect_ex(('127.0.0.1',port))==0:
                raise RuntimeError(f'Port {port} already busy; refusing a stale server')
        log=out/(name+'.server.log')
        with log.open('wb') as stream:
            p=subprocess.Popen(cmd,stdout=stream,stderr=subprocess.STDOUT,env={**env,**extraenv},start_new_session=True)
            results['systems'][name]={'command':cmd,'pid':p.pid,'queries':{}}
            start=time.perf_counter()
            try:
                # Readiness uses ASK on SPARQL servers, a compiled query on the native adapter.
                probe=next(q for q in queries if 'unsupported' not in compiled[q])
                while True:
                    if p.poll() is not None:raise RuntimeError(f'{name} exited during startup: {p.returncode}')
                    if time.perf_counter()-start>600:raise TimeoutError(name+' startup')
                    try:ask(url,queries[probe] if name=='ladybug' else 'ASK { ?s ?p ?o }');break
                    except Exception:time.sleep(1)
                if name.startswith('rdfdb'):
                    while 'building in background' in log.read_text() and 'statistics ready' not in log.read_text():
                        if p.poll() is not None:raise RuntimeError('rdfdb failed while building statistics')
                        if time.perf_counter()-start>600:raise TimeoutError('rdfdb statistics')
                        time.sleep(1)
                results['systems'][name]['ready_seconds']=time.perf_counter()-start
                for qname in sorted(queries):
                    if 'unsupported' in compiled[qname]:continue
                    rec={'samples':[]};results['systems'][name]['queries'][qname]=rec
                    try:
                        for iteration in range(args.runs+1):
                            ms,res=ask(url,queries[qname]);sig=signature(res)
                            rec['samples'].append({'warmup':iteration==0,'ms':ms,**sig});checkpoint()
                        allsig={(s['rows'],s['sha256']) for s in rec['samples']}
                        if len(allsig)!=1:raise ValueError('Answers vary across repetitions')
                        rec['median_ms']=statistics.median(s['ms'] for s in rec['samples'] if not s['warmup'])
                        rec.update(sig)
                        if name=='jena':reference[qname]=sig
                        rec['matches_jena']=sig==reference.get(qname)
                        print(name,qname,rec['median_ms'],sig['rows'],rec['matches_jena'],flush=True)
                    except Exception as exc:
                        rec['error']=str(exc);print(name,qname,'ERROR',str(exc),flush=True)
                    checkpoint()
            finally:stop(p)
        checkpoint()
    results['complete']=True;checkpoint()


if __name__=='__main__':
    p=argparse.ArgumentParser()
    for key in ('tools','queries','compiled','database','python','rdfdb','out'):p.add_argument('--'+key,type=Path,required=True)
    p.add_argument('--dataset',required=True);p.add_argument('--runs',type=int,default=5)
    run(p.parse_args())
