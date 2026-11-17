#!/usr/bin/env python3
"""Stream N-Triples into one lossless typed DuckDB triples table.

This BSBM baseline accepts named IRI subjects only; rejects blank subjects rather
than conflating them with IRIs. There are no property tables or numeric indexes.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sys
import time


def load(args):
    if Path('/tmp/rdf-bench.lock/owner').read_text().strip()!=args.lock_owner:
        raise RuntimeError('Own the benchmark lock before import')
    import duckdb
    import pyarrow as pa
    sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'ladybug'))
    from native import LINE, term
    if args.database.exists() or args.report.exists():
        raise FileExistsError('Refusing to overwrite existing baseline/evidence')
    args.database.parent.mkdir(parents=True,exist_ok=True)
    args.spill.mkdir(parents=True,exist_ok=True)
    if any(args.spill.iterdir()):
        raise ValueError('Use an empty owned spill directory')
    stats={'schema':'plain-triples-arrow-v1','duckdb':duckdb.__version__,'pyarrow':pa.__version__,
           'source':str(args.source.resolve()),'database':str(args.database.resolve()),
           'threads':args.threads,'memory_limit':args.memory_limit,'spill':str(args.spill.resolve()),
           'max_spill':args.max_spill,'batch_rows':args.batch_rows,'parsed_triples':0,'complete':False}
    schema=pa.schema([('s',pa.string()),('p',pa.string()),('o',pa.struct([
        ('kind',pa.uint8()),('value',pa.string()),('datatype',pa.string()),('lang',pa.string())]))])
    digest=hashlib.sha256()
    def batches():
        rows=[]
        with args.source.open('rb') as stream:
            for line_number,raw in enumerate(stream,1):
                digest.update(raw)
                text=raw.decode('utf8').strip()
                if not text or text.startswith('#'):continue
                match=LINE.fullmatch(text)
                if match is None:raise ValueError('Unsupported N-Triples at line '+str(line_number))
                s,p,o=(term(t) for t in match.groups())
                if s['type']!='uri' or p['type']!='uri':
                    raise ValueError('BSBM baseline requires named-IRI subjects and predicates')
                if o['type']=='bnode':
                    raise ValueError('This corpus-scoped BSBM translation requires named resources')
                rows.append({'s':s['value'],'p':p['value'],'o':{
                    'kind':{'uri':0,'bnode':1,'literal':2}[o['type']], 'value':o['value'],
                    'datatype':o.get('datatype'),'lang':o.get('xml:lang')}})
                stats['parsed_triples']+=1
                if len(rows)>=args.batch_rows:
                    yield pa.RecordBatch.from_pylist(rows,schema=schema);rows=[]
            if rows:yield pa.RecordBatch.from_pylist(rows,schema=schema)
    connection=duckdb.connect(str(args.database))
    start=time.perf_counter()
    try:
        connection.execute('SET threads=?',[args.threads])
        connection.execute('SET memory_limit=?',[args.memory_limit])
        connection.execute('SET temp_directory=?',[str(args.spill.resolve())])
        connection.execute('SET max_temp_directory_size=?',[args.max_spill])
        connection.execute('SET preserve_insertion_order=false')
        incoming=pa.RecordBatchReader.from_batches(schema,batches())
        connection.register('incoming',incoming)
        # Graph set semantics, including repeated raw statements and xsd:string equivalence.
        connection.execute('CREATE TABLE triples AS SELECT DISTINCT s,p,o FROM incoming')
        stats['unique_triples']=connection.execute('SELECT count(*) FROM triples').fetchone()[0]
        if digest.hexdigest()!=args.expected_sha256:
            raise ValueError('Source input SHA-256 differs from the pinned dataset')
        connection.execute('CHECKPOINT')
        stats.update(source_sha256=digest.hexdigest(),seconds=time.perf_counter()-start,complete=True)
    except BaseException as exc:
        stats['error']=type(exc).__name__+': '+str(exc)
        raise
    finally:
        connection.close()
        stats['database_bytes']=args.database.stat().st_size if args.database.exists() else None
        args.report.write_text(json.dumps(stats,indent=2))
    print(json.dumps(stats,indent=2))


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    for name in ('source','database','spill','report'):p.add_argument('--'+name,type=Path,required=True)
    p.add_argument('--threads',type=int,default=4);p.add_argument('--batch-rows',type=int,default=65536)
    p.add_argument('--memory-limit',default='8GB');p.add_argument('--max-spill',default='8GB')
    p.add_argument('--lock-owner',required=True)
    p.add_argument('--expected-sha256',required=True)
    args=p.parse_args()
    if min(args.threads,args.batch_rows)<1:p.error('positive threads and batch-rows required')
    load(args)
