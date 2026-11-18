#!/usr/bin/env python3
"""Stream N-Triples into one lossless typed DuckDB triples table.

This BSBM baseline accepts named IRI subjects only; rejects blank subjects rather
than conflating them with IRIs. There are no property tables or numeric indexes.
"""
import argparse
import hashlib
import json
import os
import resource
import shutil
import threading
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
    if (args.database.exists() or args.report.exists() or
            (getattr(args,'progress',None) and args.progress.exists())):
        raise FileExistsError('Refusing to overwrite existing baseline/evidence')
    if shutil.disk_usage(args.database.parent if args.database.parent.exists() else Path('/private/tmp')).free < 40*1024**3:
        raise RuntimeError('Shared-host free space is below the 40 GiB floor')
    args.database.parent.mkdir(parents=True,exist_ok=True)
    args.spill.mkdir(parents=True,exist_ok=True)
    if any(args.spill.iterdir()):
        raise ValueError('Use an empty owned spill directory')
    stats={'schema':'plain-triples-arrow-v1','duckdb':duckdb.__version__,'pyarrow':pa.__version__,
           'source':str(args.source.resolve()),'database':str(args.database.resolve()),
           'threads':args.threads,'memory_limit':args.memory_limit,'spill':str(args.spill.resolve()),
           'max_spill':args.max_spill,'batch_rows':args.batch_rows,'parsed_triples':0,'source_bytes_read':0,'complete':False,'phase':'starting',
           'progress_errors':[],
           'spill_peak_sampled_bytes':0,'spill_sample_seconds':0.2,'spill_sampling_errors':[]}
    schema=pa.schema([('s',pa.string()),('p',pa.string()),('o',pa.struct([
        ('kind',pa.uint8()),('value',pa.string()),('datatype',pa.string()),('lang',pa.string())]))])
    digest=hashlib.sha256()
    def batches():
        rows=[]
        with args.source.open('rb') as stream:
            for line_number,raw in enumerate(stream,1):
                digest.update(raw)
                stats['source_bytes_read']+=len(raw)
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
    stop=threading.Event()
    def sample_spill():
        total=0
        try:
            for path in args.spill.rglob('*'):
                try:
                    if path.is_file():total+=path.stat().st_size
                except FileNotFoundError:pass  # DuckDB can remove a file during sampling.
            stats['spill_peak_sampled_bytes']=max(stats['spill_peak_sampled_bytes'],total)
        except OSError as exc:
            stats['spill_sampling_errors'].append(type(exc).__name__+': '+str(exc))
    def progress():
        if getattr(args,'progress',None):
            snapshot=dict(stats, elapsed_seconds=time.perf_counter()-start,
                          progress_scope='parsed input only; not a resumable or committed checkpoint')
            temporary=args.progress.with_suffix(args.progress.suffix+'.tmp')
            temporary.write_text(json.dumps(snapshot,indent=2))
            temporary.replace(args.progress)
    def monitor():
        last=0
        while not stop.wait(stats['spill_sample_seconds']):
            sample_spill()
            if time.monotonic()-last>=2:
                try:progress()
                except OSError as exc:stats['progress_errors'].append(str(exc))
                last=time.monotonic()
    start=time.perf_counter()
    sampler=threading.Thread(target=monitor,daemon=True);sampler.start()
    try:
        connection.execute('SET threads=?',[args.threads])
        connection.execute('SET memory_limit=?',[args.memory_limit])
        connection.execute('SET temp_directory=?',[str(args.spill.resolve())])
        connection.execute('SET max_temp_directory_size=?',[args.max_spill])
        connection.execute('SET preserve_insertion_order=false')
        incoming=pa.RecordBatchReader.from_batches(schema,batches())
        connection.register('incoming',incoming)
        # Graph set semantics, including repeated raw statements and xsd:string equivalence.
        stats['phase']='parsing-and-distinct';progress()
        connection.execute('CREATE TABLE triples AS SELECT DISTINCT s,p,o FROM incoming')
        stats['phase']='count-and-checkpoint';progress()
        stats['unique_triples']=connection.execute('SELECT count(*) FROM triples').fetchone()[0]
        if digest.hexdigest()!=args.expected_sha256:
            raise ValueError('Source input SHA-256 differs from the pinned dataset')
        if stats['progress_errors']:raise RuntimeError('Live progress writes failed')
        connection.execute('CHECKPOINT')
        stats.update(source_sha256=digest.hexdigest(),seconds=time.perf_counter()-start,complete=True)
    except BaseException as exc:
        stats['error']=type(exc).__name__+': '+str(exc)
        raise
    finally:
        stop.set();sampler.join();sample_spill()
        connection.close()
        peak=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
        stats['process_peak_rss_bytes']=peak if sys.platform=='darwin' else peak*1024
        stats['peak_rss_scope']='whole importer process, including Python, Arrow and DuckDB'
        stats['spill_metric_scope']='sampled file lengths, not allocated blocks or guaranteed instantaneous peak'
        stats['free_disk_bytes_after']=shutil.disk_usage(args.database.parent).free
        stats['database_bytes']=args.database.stat().st_size if args.database.exists() else None
        stats['phase']='complete' if stats['complete'] else 'failed'
        args.report.write_text(json.dumps(stats,indent=2))
        progress()
    print(json.dumps(stats,indent=2))


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    for name in ('source','database','spill','report'):p.add_argument('--'+name,type=Path,required=True)
    p.add_argument('--progress',type=Path,help='Atomic live status; not a resumable checkpoint')
    p.add_argument('--threads',type=int,default=4);p.add_argument('--batch-rows',type=int,default=65536)
    p.add_argument('--memory-limit',default='8GB');p.add_argument('--max-spill',default='8GB')
    p.add_argument('--lock-owner',required=True)
    p.add_argument('--expected-sha256',required=True)
    args=p.parse_args()
    if min(args.threads,args.batch_rows)<1:p.error('positive threads and batch-rows required')
    load(args)
