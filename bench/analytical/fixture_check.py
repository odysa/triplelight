#!/usr/bin/env python3
"""Actual BI support matrix on a small graph; no performance score is emitted."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import subprocess
import time

from bi_sql import OUTPUT, translate
from qualify import qualify, check_order

XSD='http://www.w3.org/2001/XMLSchema#'


def duck_result(number, rows):
    specs=OUTPUT[number];bindings=[]
    for row in rows:
        if len(row)!=len(specs):raise ValueError('SQL projection width differs')
        binding={}
        for (name,kind),value in zip(specs,row):
            if value is None:continue
            if kind=='iri':binding[name]={'type':'uri','value':value}
            elif kind=='integer':binding[name]={'type':'literal','value':str(int(value)),'datatype':XSD+'integer'}
            else:
                lexical='NaN' if math.isnan(value) else 'INF' if value==math.inf else '-INF' if value==-math.inf else repr(value)
                binding[name]={'type':'literal','value':lexical,'datatype':XSD+'float'}
        bindings.append(binding)
    return {'head':{'vars':[name for name,_ in specs]},'results':{'bindings':bindings}}


def check(args):
    if Path('/tmp/rdf-bench.lock/owner').read_text().strip()!=args.lock_owner:
        raise RuntimeError('Own the benchmark lock for actual engine fixtures')
    import duckdb
    import rdfdb
    manifest=json.loads((args.fixture/'manifest.json').read_text())
    out=args.fixture/'qualification';out.mkdir(exist_ok=False)
    matrix={'schema':'actual-bi-fixture-v1','rdfdb_source':args.rdfdb_source,'duckdb':duckdb.__version__,
            'data_sha256':hashlib.sha256((args.fixture/'data.nt').read_bytes()).hexdigest(),
            'queries':{},'complete':False}
    db=rdfdb.Database(str(args.fixture/'rdfdb'),initial=str(args.fixture/'data.nt'))
    deadline=time.monotonic()+60
    while not db.stats_ready():
        if time.monotonic()>deadline:raise TimeoutError('fixture statistics')
        time.sleep(.01)
    connection=duckdb.connect(str(args.fixture/'duck.db'),read_only=True)
    try:
        for q in manifest['queries']:
            n=q['template'];path=args.fixture/'queries'/q['file'];source=path.read_text()
            entry={'query_sha256':hashlib.sha256(source.encode()).hexdigest()};matrix['queries'][str(n)]=entry
            command=[str(args.jena/'bin/arq'),'--data='+str(args.fixture/'data.nt'),'--query='+str(path),'--results=JSON']
            with (out/f'q{n}-jena.json').open('wb') as dest,(out/f'q{n}-jena.stderr').open('wb') as err:
                subprocess.run(command,stdout=dest,stderr=err,check=True,timeout=60)
            reference=json.loads((out/f'q{n}-jena.json').read_text())
            entry['jena_rows']=len(reference['results']['bindings'])
            entry['jena_order']=check_order(n,reference)
            for system in ('duckdb','rdfdb'):
                try:
                    if system=='duckdb':
                        sql=translate(n,q['parameters']);(out/f'q{n}.sql').write_text(sql)
                        actual=duck_result(n,connection.execute(sql).fetchall())
                    else:actual=json.loads(db.query_json(source))
                    (out/f'q{n}-{system}.json').write_text(json.dumps(actual,indent=2))
                    entry[system]={'qualification':qualify(n,actual,reference),'order':check_order(n,actual),'status':'pass'}
                    if n==6 and system=='duckdb':
                        got={r['reviewer']['value']:float(r['reviewerAvgScore']['value']) for r in actual['results']['bindings']}
                        if got!=manifest['expected_q6_reviewers']:raise ValueError('q6 fixture has unexpected semantics')
                except Exception as exc:entry[system]={'status':'fail','error':type(exc).__name__+': '+str(exc)}
            print('q'+str(n),entry['duckdb']['status'],entry['rdfdb']['status'],flush=True)
            (out/'matrix.json').write_text(json.dumps(matrix,indent=2))
        matrix['complete']=True
    finally:
        db.close();connection.close();(out/'matrix.json').write_text(json.dumps(matrix,indent=2))


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--fixture',type=Path,required=True);p.add_argument('--jena',type=Path,required=True)
    p.add_argument('--rdfdb-source',required=True);p.add_argument('--lock-owner',required=True)
    check(p.parse_args())
