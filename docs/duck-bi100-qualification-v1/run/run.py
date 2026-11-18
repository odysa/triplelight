import hashlib,json,subprocess,sys,time
from pathlib import Path
stage=Path(__file__).resolve().parent;root=stage.parent
owner='Jianlin-Su task25 100M DuckDB output qualification'
lock=Path('/tmp/rdf-bench.lock')
assert (lock/'owner').read_text().strip()==owner

def sha(path):
    digest=hashlib.sha256()
    with path.open('rb') as stream:
        while chunk:=stream.read(8*1024*1024):digest.update(chunk)
    return digest.hexdigest()
state={'complete_requests':False,'engine_qualified_strict':False,'started_unix':time.time()}
database=root/'duck100-import-v2/duck.db'
try:
    state['database_sha256_before']=sha(database)
    command=[sys.executable,str(stage/'bench-source/workload_check.py'),'--system','duckdb',
      '--workload',str(root/'bsbm100-preparation-v1/workload'),'--reference',str(root/'bsbm100-reference-v1/reference'),
      '--database',str(database),'--import-report',str(root/'duck100-import-v2/report.json'),
      '--out',str(stage/'queries'),'--dataset-sha256','6e4ef939f05b15677802d17257096de9da32a452fe33dd40c07d0b0537ee430d',
      '--numeric-inputs',str(root/'numeric-freeze-100m-v1'),'--numeric-freeze-sha256','63e127b60ac8de510a172748108639f0834e17bc3772e1d16ce226404aa58cca',
      '--lock-owner',owner,'--threads','4','--memory-limit','8GB','--max-spill','8GB',
      '--runtime-source','Plain lossless typed triples, DuckDB 2.0.0, untimed native SQL qualification']
    supervised=[sys.executable,str(stage/'bench-source/supervise_import.py'),'--root',str(root),
      '--receipt',str(stage/'supervisor.json'),'--owner',owner,'--',*command]
    (stage/'command.json').write_text(json.dumps(supervised,indent=2))
    child=subprocess.run(supervised);state['client_exit']=child.returncode
    if child.returncode:raise RuntimeError('Qualification process did not finish cleanly')
    matrix=json.loads((stage/'queries/matrix.json').read_text())
    state.update(complete_requests=matrix['complete'] and len(matrix['queries'])==60,
                 engine_qualified_strict=matrix['all_requested_pass'],
                 strict_pass=sum(q['status']=='pass' for q in matrix['queries'].values()))
    state['database_sha256_after']=sha(database)
    assert state['database_sha256_before']==state['database_sha256_after']
    for name,digest in json.loads((stage/'bench-source/source-sha256.json').read_text()).items():
        assert sha(stage/'bench-source'/name)==digest
    state['database_and_sources_unchanged']=True
except BaseException as exc:state['error']=type(exc).__name__+': '+str(exc)
finally:
    state['ended_unix']=time.time();(stage/'status.json').write_text(json.dumps(state,indent=2))
    if (lock/'owner').read_text().strip()==owner:(lock/'owner').unlink();lock.rmdir()
if not state['complete_requests']:sys.exit(1)
