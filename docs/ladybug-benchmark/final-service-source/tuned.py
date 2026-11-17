#!/usr/bin/env python3
"""Secondary LUBM mapping: exact type-signature tables and literal list columns.

A type signature retains *all* explicit types, including multiply typed nodes.
This is a specialized mapping/translator, not a general SPARQL implementation.
"""
import argparse
from array import array
import csv
import hashlib
import itertools
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

import native

csv.field_size_limit(32*1024**2)


def rows(path):
    with path.open(newline='') as f:yield from csv.reader(f)


def prepare(source,out):
    import pyarrow as pa
    import pyarrow.parquet as pq
    start=time.perf_counter();out.mkdir(parents=True,exist_ok=False)
    old=json.loads((source/'manifest.json').read_text())
    literal_bases={r['table'][:-1] for r in old['relationships'] if r['target']=='Literal'}
    resource_bases={r['table'][:-1] for r in old['relationships'] if r['target']=='Resource'}
    if literal_bases & resource_bases:raise ValueError('Tuned translator requires predicates with unmixed resource/literal ranges')
    typebase=old['predicates'][native.RDF+'type']
    typefile=source/(typebase+'R.csv')
    classids=sorted({int(o) for s,o in rows(typefile)})
    if len(classids)>63:raise ValueError('Type signatures exceed this implementation’s 63-bit bound')
    bits={ident:1<<i for i,ident in enumerate(classids)}
    masks=array('Q',[0])*(old['counts']['resource_nodes']+old['counts']['literal_nodes'])
    for s,o in rows(typefile):masks[int(s)] |= bits[int(o)]
    classes={int(i):value for i,term,value in rows(source/'Resource.csv') if int(i) in bits}
    signatures=sorted({masks[int(i)] for i,_,_ in rows(source/'Resource.csv')})
    labels={mask:f'T{i}' for i,mask in enumerate(signatures)}
    literal_terms=['']*len(masks)
    for row in rows(source/'Literal.csv'):literal_terms[int(row[0])]=row[1]
    literal_rels=[r for r in old['relationships'] if r['target']=='Literal']
    columns=sorted(r['table'][:-1] for r in literal_rels)
    props=out/'properties.tsv'
    with props.open('w') as f:
        for rel in literal_rels:
            col=rel['table'][:-1]
            for s,o in rows(source/(rel['table']+'.csv')):f.write(f'{s}\t{col}\t{o}\n')
    sorted_props=out/'properties.sorted.tsv'
    with sorted_props.open('wb') as f:
        subprocess.run(['sort','-S','256M','-T',str(out),'-t','\t','-k1,1n','-k2,2',str(props)],stdout=f,check=True,env={**os.environ,'LC_ALL':'C'})
    props.unlink()
    schema=pa.schema([('id',pa.int64()),('term',pa.string()),('value',pa.string())]+[(col,pa.list_(pa.string())) for col in columns])
    handles={};buffers={};nodecounts={label:0 for label in labels.values()}
    for label in labels.values():
        handles[label]=pq.ParquetWriter(out/(label+'.parquet'),schema,compression='snappy');buffers[label]=[]
    def flush(label):
        batch=buffers[label]
        if batch:
            data={field.name:[row[i] for row in batch] for i,field in enumerate(schema)}
            handles[label].write_table(pa.Table.from_pydict(data,schema=schema));batch.clear()
    type_count=0;literal_count=0
    try:
        with sorted_props.open() as f:
            groups=itertools.groupby((line.rstrip('\n').split('\t') for line in f),lambda row:int(row[0]))
            current=next(groups,None)
            for ident,serialized,value in rows(source/'Resource.csv'):
                ident=int(ident);colvalues={col:[] for col in columns}
                if current is not None and current[0]<ident:raise ValueError('Property subject missing from resource table')
                if current is not None and current[0]==ident:
                    for _,col,obj in current[1]:colvalues[col].append(literal_terms[int(obj)]);literal_count+=1
                    current=next(groups,None)
                mask=masks[ident];label=labels[mask];nodecounts[label]+=1;type_count+=mask.bit_count()
                buffers[label].append([ident,serialized,value]+[colvalues[col] for col in columns])
                if len(buffers[label])>=10000:flush(label)
            if current is not None:raise ValueError('Unconsumed literal properties')
    finally:
        for label in handles:
            flush(label);handles[label].close()
    sorted_props.unlink();del literal_terms
    relhandles={};relcounts={};rels={}
    try:
        for oldrel in old['relationships']:
            if oldrel['target']!='Resource' or oldrel['table']==typebase+'R':continue
            pred=oldrel['table'][:-1]
            for s,o in rows(source/(oldrel['table']+'.csv')):
                source_label=labels[masks[int(s)]];target_label=labels[masks[int(o)]]
                table=pred+'_'+source_label+'_'+target_label
                if table not in relhandles:
                    relhandles[table]=(out/(table+'.csv')).open('w')
                    relcounts[table]=0;rels[table]={'table':table,'predicate':pred,'source':source_label,'target':target_label}
                relhandles[table].write(s+','+o+'\n');relcounts[table]+=1
    finally:
        for f in relhandles.values():f.close()
    with (out/'resource-masks.u64').open('wb') as f:masks.tofile(f)
    total=type_count+literal_count+sum(relcounts.values())
    assert total==old['counts']['unique_triples'],(total,old['counts'])
    manifest={'mapping':'exact-type-signatures-literal-columns-v1','source_csv':str(source.resolve()),'node_format':'parquet','pyarrow_version':pa.__version__,
              'input':old['input'],'input_sha256':old['input_sha256'],'predicates':old['predicates'],'mask_byteorder':sys.byteorder,
              'types':{classes[i]:bits[i] for i in classids},'signatures':{label:mask for mask,label in labels.items()},
              'nodecounts':nodecounts,'literal_columns':columns,'relationships':list(rels.values()),
              'relationship_counts':relcounts,'counts':{'resource_nodes':sum(nodecounts.values()),'rdf_type_triples':type_count,'literal_triples':literal_count,'resource_triples':sum(relcounts.values()),'unique_triples':total},
              'prepare_seconds':time.perf_counter()-start}
    (out/'manifest.json').write_text(json.dumps(manifest,indent=2));print(json.dumps(manifest),flush=True)


def load(csvdir,dbpath,memory_mb,threads):
    import ladybug as lb
    manifest=json.loads((csvdir/'manifest.json').read_text());start=time.perf_counter()
    db=lb.Database(str(dbpath),buffer_pool_size=memory_mb*1024**2,max_num_threads=threads);conn=lb.Connection(db)
    def copy(table,nodes=False):
        suffix='.parquet' if nodes else '.csv'
        options='' if nodes else ' (HEADER=false)'
        conn.execute(f'COPY {table} FROM '+json.dumps(str((csvdir/(table+suffix)).resolve()))+options)
    observed_nodes=0;observed_type=0;observed_literals=0;observed_edges=0
    for label,mask in manifest['signatures'].items():
        columns=['id INT64','term STRING','value STRING']+[col+' STRING[]' for col in manifest['literal_columns']]+['PRIMARY KEY(id)']
        conn.execute(f'CREATE NODE TABLE {label}('+', '.join(columns)+')')
        copy(label,True)
        count=conn.execute(f'MATCH (n:{label}) RETURN count(*)').get_next()[0]
        assert count==manifest['nodecounts'][label]
        observed_nodes+=count;observed_type+=count*int(mask).bit_count()
        expr=' + '.join('size(n.'+col+')' for col in manifest['literal_columns'])
        if expr:observed_literals+=int(conn.execute(f'MATCH (n:{label}) RETURN sum({expr})').get_next()[0] or 0)
    for rel in manifest['relationships']:
        conn.execute(f'CREATE REL TABLE {rel["table"]}(FROM {rel["source"]} TO {rel["target"]})');copy(rel['table'])
        count=conn.execute(f'MATCH ()-[r:{rel["table"]}]->() RETURN count(*)').get_next()[0]
        assert count==manifest['relationship_counts'][rel['table']]
        observed_edges+=count
    assert observed_type+observed_literals+observed_edges==manifest['counts']['unique_triples'],(observed_nodes,observed_type,observed_literals,observed_edges,manifest['counts'])
    conn.execute('CHECKPOINT')
    rec={'load_seconds':time.perf_counter()-start,'resource_nodes':observed_nodes,'rdf_type_triples':observed_type,'literal_triples':observed_literals,'resource_triples':observed_edges,'unique_triples':observed_type+observed_literals+observed_edges,'memory_mb':memory_mb,'threads':threads}
    (csvdir/'load.json').write_text(json.dumps(rec,indent=2));print(json.dumps(rec),flush=True);conn.close();db.close()


def compile_query(query,manifest,resources,masks):
    prefixes=dict(re.findall(r'PREFIX\s+([\w-]+):\s*<([^>]+)>',query,re.I))
    q=re.sub(r'PREFIX\s+[\w-]+:\s*<[^>]+>','',query,flags=re.I).strip()
    m=re.fullmatch(r'SELECT\s+((?:\?\w+\s*)+)WHERE\s*\{(.*)\}\s*',q,re.I|re.S)
    if not m:raise ValueError('Tuned translator currently accepts plain LUBM SELECT BGP only')
    projection=m[1].split();body=m[2];tokens=[];pos=0
    token=re.compile(r'<[^>]+>|\?\w+|[\w-]+:[\w-]+|\ba\b|\.')
    while pos<len(body):
        if body[pos].isspace():pos+=1;continue
        t=token.match(body,pos)
        if not t:raise ValueError('Unsupported tuned-query syntax')
        tokens.append(t[0]);pos=t.end()
    triples=[]
    while tokens:
        if len(tokens)<3:raise ValueError('Incomplete triple')
        triples.append(tokens[:3]);tokens=tokens[3:]
        if tokens and tokens.pop(0)!='.':raise ValueError('Missing separator')
    def expand(t):
        if t=='a':return native.RDF+'type'
        if t.startswith('<'):return native.term(t)['value']
        a,b=t.split(':',1);return prefixes[a]+b
    sigs=manifest['signatures'];alllabels=set(sigs);mask_to_label={int(mask):label for label,mask in sigs.items()}
    allowed={};aliases={};constraints=[]
    def node(t):
        if t not in aliases:
            aliases[t]='v_'+t[1:] if t.startswith('?') else 'c_'+str(len(aliases))
            if t.startswith('?'):allowed[t]=set(alllabels)
            else:
                ident=resources.get(native.key({'type':'uri','value':expand(t)}),-1)
                allowed[t]={mask_to_label[masks[ident]]} if ident>=0 else set(alllabels)
                constraints.append(aliases[t]+'.id = '+str(ident))
        return aliases[t]
    remaining=[]
    for s,p,o in triples:
        pred=expand(p)
        if pred==native.RDF+'type':
            if o.startswith('?'):raise ValueError('Variable rdf:type object not translated')
            node(s);bit=manifest['types'].get(expand(o),0)
            allowed[s]&={label for label,mask in sigs.items() if int(mask)&bit}
        else:remaining.append((s,pred,o))
    literalvars={};unwinds=[];edges=[];seen=set()
    for s,pred,o in remaining:
        a=node(s);base=manifest['predicates'].get(pred)
        if base in manifest['literal_columns']:
            if not o.startswith('?') or o in literalvars:raise ValueError('Tuned literal joins/constants require an additional translation')
            literalvars[o]='l_'+o[1:];unwinds.append(f'UNWIND {a}.{base} AS {literalvars[o]}')
        else:
            b=node(o);edges.append((s,base,o));seen|={s,o}
    if set(literalvars)&set(aliases):raise ValueError('Mixed resource/literal variable not translated')
    def pattern(t):
        if not allowed[t]:raise ValueError('Empty type signature intersection')
        return '('+aliases[t]+':'+ '|'.join(sorted(allowed[t]))+')'
    patterns=[]
    for s,base,o in edges:
        tables=[r['table'] for r in manifest['relationships'] if r['predicate']==base and r['source'] in allowed[s] and r['target'] in allowed[o]]
        if not tables:raise ValueError('No matching typed relationship table')
        patterns.append(pattern(s)+'-[:'+'|'.join(tables)+']->'+pattern(o))
    patterns += [pattern(t) for t in aliases if t not in seen]
    cypher='MATCH '+', '.join(patterns)
    if constraints:cypher+=' WHERE '+' AND '.join(constraints)
    if unwinds:cypher+=' '+' '.join(unwinds)
    returns=[]
    for v in projection:
        expr=literalvars.get(v,aliases.get(v,'')+'.term')
        if expr=='.term':raise ValueError('Unbound projection')
        returns.append(expr+' AS '+v[1:])
    cypher+=' RETURN '+', '.join(returns)
    return {'vars':[v[1:] for v in projection],'cypher':cypher}


def compile_queries(csvdir,qdir,out):
    manifest=json.loads((csvdir/'manifest.json').read_text());source=Path(manifest['source_csv'])
    if manifest['mask_byteorder']!=sys.byteorder:raise ValueError('Regenerate mask file for this host byte order')
    resources=json.loads((source/'resource-ids.json').read_text());masks=array('Q')
    with (csvdir/'resource-masks.u64').open('rb') as f:masks.frombytes(f.read())
    result={}
    for path in qdir.glob('*.rq'):
        query=path.read_text()
        try:rec=compile_query(query,manifest,resources,masks)
        except ValueError as e:rec={'unsupported':str(e)}
        result[path.stem]={'source_sha256':hashlib.sha256(query.encode()).hexdigest(),**rec}
    out.write_text(json.dumps(result,indent=2))


if __name__=='__main__':
    p=argparse.ArgumentParser();sub=p.add_subparsers(dest='mode',required=True)
    s=sub.add_parser('prepare');s.add_argument('source',type=Path);s.add_argument('out',type=Path)
    s=sub.add_parser('load');s.add_argument('csvdir',type=Path);s.add_argument('dbpath',type=Path);s.add_argument('--memory-mb',type=int,default=10240);s.add_argument('--threads',type=int,default=4)
    s=sub.add_parser('compile');s.add_argument('csvdir',type=Path);s.add_argument('qdir',type=Path);s.add_argument('out',type=Path)
    args=vars(p.parse_args());mode=args.pop('mode');globals()[{'compile':'compile_queries'}.get(mode,mode)](**args)
