#!/usr/bin/env python3
"""Untimed qualification of exported BI parameters, one engine per process.

Run Jena first, then DuckDB and rdfdb serially. This emits no performance score.
Only attempted query files are marked complete; a partial run is explicitly
labelled and never represents a complete weighted mix.
"""
import argparse
import hashlib
import json
from pathlib import Path
import time
import urllib.parse
import urllib.request

from bi_sql import translate
from fixture_check import duck_result, XSD
from qualify import qualify, check_order, q7_allowed_products


def ask(endpoint, query, timeout):
    data=urllib.parse.urlencode({'query':query}).encode()
    request=urllib.request.Request(endpoint,data=data,headers={
        'Accept':'application/sparql-results+json','Content-Type':'application/x-www-form-urlencoded'})
    with urllib.request.urlopen(request,timeout=timeout) as response:return json.load(response)


def diagnostic_query(parameters):
    return f'''PREFIX bsbm: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/>
SELECT ?product ?offerCount (EXISTS {{ ?offer bsbm:product ?product . ?offer bsbm:vendor ?vendor . ?vendor bsbm:country {parameters['Country']} }} AS ?blocked)
WHERE {{ {{ SELECT ?product (COUNT(?offer) AS ?offerCount) {{ ?product a {parameters['ProductType']} . ?offer bsbm:product ?product }} GROUP BY ?product }} }}'''


def diagnostics(result):
    rows=[]
    for row in result['results']['bindings']:
        if row['product']['type']!='uri' or row['offerCount'].get('datatype')!=XSD+'integer':
            raise ValueError('Unexpected diagnostic product/count datatype')
        b=row['blocked']
        if b.get('datatype')!=XSD+'boolean' or b['value'] not in ('true','false','1','0'):
            raise ValueError('Unexpected diagnostic boolean')
        rows.append({'product':row['product']['value'],'offerCount':int(row['offerCount']['value']),
                     'blocked':b['value'] in ('true','1')})
    return rows


def run(args):
    def lock():
        if Path('/tmp/rdf-bench.lock/owner').read_text().strip()!=args.lock_owner:
            raise RuntimeError('Own the shared benchmark lock throughout qualification')
    lock()
    manifest=json.loads((args.workload/'manifest.json').read_text())
    if manifest['source_data_sha256']!=args.dataset_sha256:raise ValueError('Workload/dataset identity differs')
    requested=manifest['queries'][:args.limit] if args.limit else manifest['queries']
    args.out.mkdir(parents=True,exist_ok=False)
    matrix={'schema':'actual-bi-parameter-qualification-v1','system':args.system,'runtime_source':args.runtime_source,
            'dataset_sha256':args.dataset_sha256,'manifest_sha256':hashlib.sha256((args.workload/'manifest.json').read_bytes()).hexdigest(),
            'requested_files':[q['file'] for q in requested],'partial_workload':len(requested)!=len(manifest['queries']),
            'queries':{},'complete':False}
    frozen_inputs=None
    if args.numeric_inputs:
        from envelope_qualify import FrozenInputs
        frozen_inputs=FrozenInputs(args.numeric_inputs,args.numeric_freeze_sha256,args.dataset_sha256,matrix['manifest_sha256'])
        matrix['numeric_freeze_sha256']=frozen_inputs.freeze_sha256
        matrix['numeric_policy']='exact-anchor-binary32-v1'
    else:matrix['numeric_policy']='original-four-binary32-ulps'
    connection=db=None
    try:
        if args.system=='duckdb':
            import duckdb
            report=json.loads(args.import_report.read_text())
            if not report['complete'] or report['source_sha256']!=args.dataset_sha256:
                raise ValueError('DuckDB import is incomplete or from another dataset')
            spill=args.spill or args.out/'spill'
            spill.mkdir(parents=True,exist_ok=True)
            if any(spill.iterdir()):raise ValueError('Use an empty owned query spill directory')
            connection=duckdb.connect(str(args.database),read_only=True)
            connection.execute('SET threads=?',[args.threads]);connection.execute('SET memory_limit=?',[args.memory_limit])
            connection.execute('SET temp_directory=?',[str(spill.resolve())])
            connection.execute('SET max_temp_directory_size=?',[args.max_spill])
            matrix['runtime_version']=duckdb.__version__
            matrix['configuration']={'threads':args.threads,'memory_limit':args.memory_limit,
                                     'max_spill':args.max_spill,'spill':str(spill.resolve())}
        elif args.system=='rdfdb':
            import rdfdb
            matrix['runtime_package']=rdfdb.__file__
            db=rdfdb.Database(str(args.database),initial=str(args.source) if args.source else None)
            deadline=time.monotonic()+args.timeout
            while not db.stats_ready():
                if time.monotonic()>deadline:raise TimeoutError('statistics readiness')
                time.sleep(.05)
        for q in requested:
            lock();name=q['file'];n=q['template'];source=(args.workload/'queries'/name).read_text()
            if hashlib.sha256(source.encode()).hexdigest()!=q['sha256']:raise ValueError('Query hash mismatch: '+name)
            stem=Path(name).stem;entry={'template':n,'sha256':q['sha256']};matrix['queries'][name]=entry
            try:
                diagnostic=None
                if args.system=='jena':
                    actual=ask(args.endpoint,source,args.timeout)
                    # Preserve raw reference output even if its numeric envelope fails.
                    (args.out/(stem+'.json')).write_text(json.dumps(actual,indent=2))
                    if n==7:
                        text=diagnostic_query(q['parameters']);(args.out/(stem+'-diagnostic.rq')).write_text(text)
                        diagnostic=ask(args.endpoint,text,args.timeout)
                        (args.out/(stem+'-diagnostic.json')).write_text(json.dumps(diagnostic,indent=2))
                        q7_allowed_products([r['product']['value'] for r in actual['results']['bindings']],diagnostics(diagnostic))
                else:
                    reference=json.loads((args.reference/(stem+'.json')).read_text())
                    if n==7:diagnostic=json.loads((args.reference/(stem+'-diagnostic.json')).read_text())
                    if args.system=='duckdb':
                        sql=translate(n,q['parameters']);(args.out/(stem+'.sql')).write_text(sql)
                        actual=duck_result(n,connection.execute(sql).fetchall())
                        if n==7:
                            got=connection.execute(translate(n,q['parameters'],q7_diagnostic=True)).fetchall()
                            expected=diagnostics(diagnostic)
                            if sorted(got)!=sorted((r['product'],r['offerCount'],r['blocked']) for r in expected):
                                raise ValueError('Full q7 diagnostic differs from Jena')
                    else:actual=json.loads(db.query_json(source))
                    (args.out/(stem+'.json')).write_text(json.dumps(actual,indent=2))
                    if n==7:
                        if actual['head']['vars']!=['product'] or any(r['product']['type']!='uri' for r in actual['results']['bindings']):
                            raise ValueError('q7 result shape/type differs')
                        entry['qualification']={'q7_cutoff':q7_allowed_products([r['product']['value'] for r in actual['results']['bindings']],diagnostics(diagnostic))}
                    elif frozen_inputs is not None and n in (4,5,6):
                        from envelope_qualify import qualify_enveloped
                        entry['qualification']=qualify_enveloped(n,actual,reference,frozen_inputs.for_instance(q))
                    else:entry['qualification']=qualify(n,actual,reference)
                if args.system=='jena' and frozen_inputs is not None and n in (4,5,6):
                    from envelope_qualify import qualify_enveloped
                    entry['qualification']=qualify_enveloped(n,actual,actual,frozen_inputs.for_instance(q))
                (args.out/(stem+'.json')).write_text(json.dumps(actual,indent=2))
                entry.update(status='pass',rows=len(actual['results']['bindings']),order=check_order(n,actual))
            except Exception as exc:entry.update(status='fail',error=type(exc).__name__+': '+str(exc))
            (args.out/'matrix.json').write_text(json.dumps(matrix,indent=2));print(name,entry['status'],flush=True)
        matrix['complete']=True
        matrix['all_requested_pass']=all(v['status']=='pass' for v in matrix['queries'].values())
    finally:
        if db is not None:db.close()
        if connection is not None:connection.close()
        (args.out/'matrix.json').write_text(json.dumps(matrix,indent=2))


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--system',choices=['jena','duckdb','rdfdb'],required=True)
    for name in ['workload','out']:p.add_argument('--'+name,type=Path,required=True)
    for name in ['database','import-report','reference','source']:p.add_argument('--'+name,type=Path)
    for name in ['lock-owner','dataset-sha256','runtime-source']:p.add_argument('--'+name,required=True)
    p.add_argument('--numeric-inputs',type=Path);p.add_argument('--numeric-freeze-sha256')
    p.add_argument('--endpoint');p.add_argument('--timeout',type=int,default=120)
    p.add_argument('--limit',type=int,default=0);p.add_argument('--threads',type=int,default=4)
    p.add_argument('--memory-limit',default='8GB');p.add_argument('--max-spill',default='8GB')
    p.add_argument('--spill',type=Path)
    args=p.parse_args()
    if args.limit<0 or min(args.timeout,args.threads)<1:p.error('positive timeout/threads and nonnegative limit required')
    if bool(args.numeric_inputs)!=bool(args.numeric_freeze_sha256):p.error('numeric inputs and freeze hash must be supplied together')
    if args.system=='jena' and not args.endpoint:p.error('Jena needs --endpoint')
    if args.system!='jena' and (not args.database or not args.reference):p.error('candidate needs --database and --reference')
    if args.system=='duckdb' and not args.import_report:p.error('DuckDB needs --import-report')
    run(args)
