#!/usr/bin/env python3
"""Untimed Jena/rdfdb diagnostics for empty and unbound aggregate inputs.

This is separate from the eight BI templates; it supplies no DuckDB translation
and no benchmark score. Complete RDF term bags, including unbound cells, must
match the independent Jena result exactly.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import time

from qualify import qualify


def make(folder):
    folder.mkdir(parents=True, exist_ok=False)
    (folder/'data.nt').write_text(
        '<urn:a> <urn:type> <urn:Thing> .\n'
        '<urn:b> <urn:type> <urn:Thing> .\n'
        '<urn:a> <urn:value> "2"^^<http://www.w3.org/2001/XMLSchema#integer> .\n')
    projection = '''(COUNT(*) AS ?rows) (COUNT(?v) AS ?count)
(COUNT(DISTINCT ?v) AS ?distinctCount) (SUM(?v) AS ?sum)
(AVG(?v) AS ?avg) (MIN(?v) AS ?min) (MAX(?v) AS ?max)
(SAMPLE(?v) AS ?sample) (GROUP_CONCAT(?v; SEPARATOR="|") AS ?concat)'''
    patterns = {
        'mixed-bound-unbound': '?s <urn:type> <urn:Thing> OPTIONAL { ?s <urn:value> ?v }',
        'all-unbound': '?s <urn:type> <urn:Thing> OPTIONAL { ?s <urn:missing> ?v }',
        'empty-global': '?s <urn:missing> ?v',
        'empty-explicit-group': '?s <urn:missing> ?v',
    }
    cases=[]
    for name,pattern in patterns.items():
        group = ' GROUP BY ?s' if name=='empty-explicit-group' else ''
        source=f'SELECT {projection} WHERE {{ {pattern} }}{group}\n'
        (folder/(name+'.rq')).write_text(source)
        cases.append({'name':name,'query_sha256':hashlib.sha256(source.encode()).hexdigest()})
    (folder/'manifest.json').write_text(json.dumps({'scope':'jena-rdfdb-aggregate-diagnostic-v1','cases':cases},indent=2))


def check(args):
    if Path('/tmp/rdf-bench.lock/owner').read_text().strip()!=args.lock_owner:
        raise RuntimeError('Own the benchmark lock for actual engine diagnostics')
    import rdfdb
    out=args.fixture/'qualification';out.mkdir(exist_ok=False)
    manifest=json.loads((args.fixture/'manifest.json').read_text())
    matrix={'schema':manifest['scope'],'rdfdb_source':args.rdfdb_source,
            'data_sha256':hashlib.sha256((args.fixture/'data.nt').read_bytes()).hexdigest(),
            'cases':{},'complete':False}
    db=rdfdb.Database(str(args.fixture/'rdfdb'),initial=str(args.fixture/'data.nt'))
    try:
        deadline=time.monotonic()+60
        while not db.stats_ready():
            if time.monotonic()>deadline:raise TimeoutError('fixture statistics')
            time.sleep(.01)
        for case in manifest['cases']:
            name=case['name'];query=args.fixture/(name+'.rq')
            if hashlib.sha256(query.read_bytes()).hexdigest()!=case['query_sha256']:
                raise ValueError('Diagnostic query changed after generation')
            with (out/(name+'-jena.json')).open('wb') as dest,(out/(name+'-jena.stderr')).open('wb') as err:
                subprocess.run([str(args.jena/'bin/arq'),'--data='+str(args.fixture/'data.nt'),
                                '--query='+str(query),'--results=JSON'],stdout=dest,stderr=err,check=True,timeout=60)
            reference=json.loads((out/(name+'-jena.json')).read_text())
            entry={'query_sha256':case['query_sha256'],'jena_rows':len(reference['results']['bindings'])}
            matrix['cases'][name]=entry
            try:
                actual=json.loads(db.query_json(query.read_text()))
                (out/(name+'-rdfdb.json')).write_text(json.dumps(actual,indent=2))
                entry['qualification']=qualify(0,actual,reference,max_ulps=0)
                if not entry['qualification']['exact_term_bag']:raise ValueError('RDF term bags differ')
                entry['status']='pass'
            except Exception as exc:
                entry.update(status='fail',error=type(exc).__name__+': '+str(exc))
            print(name,entry['status'],flush=True)
            (out/'matrix.json').write_text(json.dumps(matrix,indent=2))
        matrix['complete']=True
    finally:
        db.close();(out/'matrix.json').write_text(json.dumps(matrix,indent=2))


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--fixture',type=Path,required=True);p.add_argument('--make',action='store_true')
    p.add_argument('--jena',type=Path);p.add_argument('--rdfdb-source');p.add_argument('--lock-owner')
    args=p.parse_args()
    if args.make:make(args.fixture)
    else:
        if not all((args.jena,args.rdfdb_source,args.lock_owner)):p.error('check needs --jena, --rdfdb-source and --lock-owner')
        check(args)
