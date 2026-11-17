#!/usr/bin/env python3
"""Prepare exact BI float-input anchors and independent Jena count diagnostics.

Prototype: no candidate outputs are read. Four explicit phases keep SQL input
collection and Jena diagnostics serial. A verified plan is an input to future
validation, not retrospective acceptance of the original four-ULP failures.
"""
import argparse
import hashlib
import json
import re
from pathlib import Path

from bi_sql import common, iri, literal, BSBM
from float_envelope import InputStats, decimal_to_binary32
from workload_check import ask

XSD='http://www.w3.org/2001/XMLSchema#'
PREFIX=f'PREFIX bsbm: <{BSBM}> PREFIX rev: <http://purl.org/stuff/rev#>\n'


def prefix(definitions):
    return 'WITH '+',\n'.join(name+' AS ('+sql+')' for name,sql in definitions.items())+'\n'


def rating_union(variable='?score'):
    return ' UNION '.join('{ ?review bsbm:rating'+str(i)+' '+variable+' }' for i in range(1,5))


def case_queries(number,parameters):
    definitions=common();sql={};jena={}
    if number in (4,5):
        product_type=parameters['ProductType']
        definitions['products']='SELECT s AS product FROM types WHERE obj='+iri(product_type)
        definitions['offers']='SELECT products.product, prod.s AS offer, price.lex AS price FROM products JOIN product prod ON prod.obj=products.product JOIN price ON price.s=prod.s'
        sql['prices']=prefix(definitions)+'SELECT product,price FROM offers'
        basic=f'?product a {product_type} . ?offer bsbm:product ?product ; bsbm:price ?price .'
        if number==5:
            jena['groups']=PREFIX+f'SELECT ?product (COUNT(*) AS ?n) WHERE {{ {basic} }} GROUP BY ?product'
        else:
            sql['features']=prefix(definitions)+'SELECT DISTINCT p.product,f.obj AS feature FROM products p JOIN productFeature f ON f.s=p.product'
            with_pattern=basic+' ?product bsbm:productFeature ?feature .'
            feature_set=f'{{ SELECT DISTINCT ?feature WHERE {{ ?p a {product_type} ; bsbm:productFeature ?feature }} }}'
            without_pattern=feature_set+' '+basic+' FILTER NOT EXISTS { ?product bsbm:productFeature ?feature }'
            jena['groups']=PREFIX+f'''SELECT ?feature ?withN ?withoutN WHERE {{
{{ SELECT ?feature (COUNT(*) AS ?withN) WHERE {{ {with_pattern} }} GROUP BY ?feature }}
{{ SELECT ?feature (COUNT(*) AS ?withoutN) WHERE {{ {without_pattern} }} GROUP BY ?feature }}
}}'''
    elif number==6:
        producer=parameters['Producer']
        definitions['ratingRaw']=' UNION ALL '.join(
            f'SELECT s AS review,o.value AS lexical,o.datatype AS datatype FROM triples WHERE p={literal(BSBM+"rating"+str(i))} AND o.kind=2' for i in range(1,5))
        base='FROM producer p JOIN reviewFor r ON r.obj=p.s JOIN ratingRaw rt ON rt.review=r.s '
        condition=' WHERE p.obj='+iri(producer)
        sql['global']=prefix(definitions)+'SELECT rt.lexical,rt.datatype '+base+condition
        sql['groups']=prefix(definitions)+'SELECT rr.obj AS reviewer,rt.lexical,rt.datatype '+base+'JOIN reviewer rr ON rr.s=r.s'+condition
        pattern=f'?product bsbm:producer {producer} . ?review bsbm:reviewFor ?product . '+rating_union()
        jena['global']=PREFIX+f'SELECT (COUNT(*) AS ?n) WHERE {{ {pattern} }}'
        jena['groups']=PREFIX+f'SELECT ?reviewer (COUNT(*) AS ?n) WHERE {{ {pattern} ?review rev:reviewer ?reviewer }} GROUP BY ?reviewer'
    else:raise ValueError('Only float-AVG templates 4/5/6 have these anchors')
    return sql,jena


def make(args):
    manifest=json.loads((args.workload/'manifest.json').read_text())
    args.out.mkdir(parents=True,exist_ok=False)
    plan={'schema':'bi-exact-float-inputs-v1','dataset_sha256':manifest['source_data_sha256'],
          'workload_manifest_sha256':hashlib.sha256((args.workload/'manifest.json').read_bytes()).hexdigest(),
          'cases':{},'instances':{},'collector_reads_candidate_outputs':False,'previously_examined_mixes':[0],'validator_frozen':False}
    for q in manifest['queries']:
        if q['template'] not in (4,5,6):continue
        key=str(q['template'])+'-'+hashlib.sha256(json.dumps(q['parameters'],sort_keys=True).encode()).hexdigest()[:20]
        plan['instances'][q['file']]={'case':key,'query_sha256':q['sha256']}
        if key in plan['cases']:continue
        sql,jena=case_queries(q['template'],q['parameters']);folder=args.out/key;folder.mkdir()
        plan['cases'][key]={'template':q['template'],'parameters':q['parameters'],'sql':{},'jena':{}}
        for kind,queries in [('sql',sql),('jena',jena)]:
            for name,text in queries.items():
                path=folder/(name+('.sql' if kind=='sql' else '.rq'));path.write_text(text)
                plan['cases'][key][kind][name]={'file':str(path.relative_to(args.out)),'sha256':hashlib.sha256(text.encode()).hexdigest()}
    (args.out/'plan.json').write_text(json.dumps(plan,indent=2))


def merge(a,b,sign=1):
    return InputStats(a.n+sign*b.n,a.total+sign*b.total,a.magnitude+sign*b.magnitude)


def lock(args):
    if Path('/tmp/rdf-bench.lock/owner').read_text().strip()!=args.lock_owner:
        raise RuntimeError('Own the shared lock for graph diagnostics')


def source_query(args,record):
    text=(args.out/record['file']).read_text()
    if hashlib.sha256(text.encode()).hexdigest()!=record['sha256']:raise ValueError('Diagnostic query changed')
    return text


def collect_sql(args,plan):
    lock(args)
    import duckdb
    report=json.loads(args.import_report.read_text())
    if not report['complete'] or report['source_sha256']!=plan['dataset_sha256']:raise ValueError('Import/workload identity differs')
    dest=args.out/'sql';dest.mkdir(exist_ok=False)
    connection=duckdb.connect(str(args.database),read_only=True)
    connection.execute('SET threads=?',[args.threads]);connection.execute("SET memory_limit='8GB'")
    spill=dest/'spill';spill.mkdir();connection.execute('SET temp_directory=?',[str(spill.resolve())]);connection.execute("SET max_temp_directory_size='8GB'")
    def rows(query):
        cursor=connection.execute(query)
        while True:
            batch=cursor.fetchmany(65536)
            if not batch:break
            yield from batch
    try:
        for key,case in plan['cases'].items():
            lock(args);number=case['template'];queries={name:source_query(args,r) for name,r in case['sql'].items()}
            groups={};global_stats=InputStats()
            if number in (4,5):
                for product,lexical in rows(queries['prices']):
                    value=decimal_to_binary32(lexical)
                    # This cohort's price envelope is restricted to nonnegative finite inputs.
                    if value<0:raise ValueError('Negative price requires separate domain qualification')
                    groups.setdefault(product,InputStats()).add(value);global_stats.add(value)
                if number==4:
                    features={}
                    for product,feature in rows(queries['features']):
                        features[feature]=merge(features.get(feature,InputStats()),groups.get(product,InputStats()))
                    record={'groups':{feature:{'with':stats.record(),'without':merge(global_stats,stats,-1).record()}
                                      for feature,stats in features.items() if stats.n and global_stats.n>stats.n},
                            'all_selected_offers':global_stats.record()}
                else:record={'groups':{product:stats.record() for product,stats in groups.items()}}
            else:
                for lexical,datatype in rows(queries['global']):
                    if datatype!=XSD+'integer' or not re.fullmatch(r'[+-]?[0-9]+',lexical.strip(' \t\r\n')):
                        raise ValueError('Rating datatype/lexical form outside verified BSBM integer profile')
                    global_stats.add(decimal_to_binary32(lexical))
                for reviewer,lexical,datatype in rows(queries['groups']):
                    if datatype!=XSD+'integer' or not re.fullmatch(r'[+-]?[0-9]+',lexical.strip(' \t\r\n')):
                        raise ValueError('Rating datatype/lexical form outside verified BSBM integer profile')
                    groups.setdefault(reviewer,InputStats()).add(decimal_to_binary32(lexical))
                record={'groups':{reviewer:stats.record() for reviewer,stats in groups.items()},'global':global_stats.record()}
            (dest/(key+'.json')).write_text(json.dumps(record,indent=2));print(key,'SQL input anchors collected',flush=True)
    finally:connection.close()


def collect_jena(args,plan):
    lock(args);dest=args.out/'jena';dest.mkdir(exist_ok=False)
    for key,case in plan['cases'].items():
        for name,record in case['jena'].items():
            lock(args);result=ask(args.endpoint,source_query(args,record),args.timeout)
            (dest/(key+'-'+name+'.json')).write_text(json.dumps(result,indent=2))
        print(key,'independent Jena counts collected',flush=True)


def count(row,name):
    term=row[name]
    if term.get('datatype')!=XSD+'integer':raise ValueError('Diagnostic count datatype differs')
    value=int(term['value'])
    if value<0:raise ValueError('Negative count')
    return value


def verify(args,plan):
    result={'schema':'bi-input-count-verification-v1','plan_sha256':hashlib.sha256((args.out/'plan.json').read_bytes()).hexdigest(),
            'cases':{},'all_counts_agree':False,'validator_frozen':False}
    for key,case in plan['cases'].items():
        exact=json.loads((args.out/'sql'/(key+'.json')).read_text())
        reference=json.loads((args.out/'jena'/(key+'-groups.json')).read_text())['results']['bindings']
        number=case['template'];variable={4:'feature',5:'product',6:'reviewer'}[number];observed={}
        for row in reference:
            if row[variable]['type']!='uri':raise ValueError('Diagnostic key is not an IRI')
            entity=row[variable]['value']
            if entity in observed:raise ValueError('Duplicate diagnostic group')
            observed[entity]=(count(row,'withN'),count(row,'withoutN')) if number==4 else count(row,'n')
        expected={entity:(v['with']['n'],v['without']['n']) if number==4 else v['n'] for entity,v in exact['groups'].items()}
        if observed!=expected:raise ValueError('Independent input-group cardinalities differ: '+key)
        if number==6:
            global_rows=json.loads((args.out/'jena'/(key+'-global.json')).read_text())['results']['bindings']
            if len(global_rows)!=1 or count(global_rows[0],'n')!=exact['global']['n']:raise ValueError('Global rating scope differs')
        result['cases'][key]={'counts_agree':True,'groups':len(expected)}
    result['all_counts_agree']=True
    (args.out/'count-verification.json').write_text(json.dumps(result,indent=2));print('All independent input-group counts agree')


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--phase',choices=['make','sql','jena','verify'],required=True);p.add_argument('--out',type=Path,required=True)
    for name in ['workload','database','import-report']:p.add_argument('--'+name,type=Path)
    p.add_argument('--endpoint');p.add_argument('--lock-owner');p.add_argument('--threads',type=int,default=4);p.add_argument('--timeout',type=int,default=120)
    args=p.parse_args()
    if args.phase=='make':
        if not args.workload:p.error('make needs --workload')
        make(args)
    else:
        plan=json.loads((args.out/'plan.json').read_text())
        if args.phase=='sql':
            if not args.database or not args.import_report or not args.lock_owner:p.error('sql needs database/import-report/lock-owner')
            collect_sql(args,plan)
        elif args.phase=='jena':
            if not args.endpoint or not args.lock_owner:p.error('jena needs endpoint/lock-owner')
            collect_jena(args,plan)
        else:verify(args,plan)
