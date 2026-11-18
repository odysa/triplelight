import hashlib,json,subprocess,sys,time
from pathlib import Path
stage=Path(__file__).resolve().parent
root=stage.parent
owner='Jianlin-Su task25 guarded 100M DuckDB reference import'
lock=Path('/tmp/rdf-bench.lock')
manifest=json.loads((stage/'manifest.json').read_text())
result={'complete':False,'start_unix':time.time()}
try:
    assert (lock/'owner').read_text().strip()==owner
    command=[sys.executable,str(stage/'bench-source/analytical/duck_load.py'),
      '--source',manifest['input']['source'],'--database',str(stage/'duck.db'),
      '--spill',str(stage/'spill'),'--report',str(stage/'report.json'),
      '--progress',str(stage/'progress.json'),'--threads','4',
      '--memory-limit','8GB','--max-spill','8GB','--lock-owner',owner,
      '--expected-sha256',manifest['input']['sha256']]
    supervised=[sys.executable,str(stage/'bench-source/analytical/supervise_import.py'),
      '--root',str(root),'--receipt',str(stage/'supervisor.json'),'--owner',owner,'--',*command]
    (stage/'command.json').write_text(json.dumps(supervised,indent=2))
    child=subprocess.run(supervised)
    result['supervisor_exit']=child.returncode
    if child.returncode:raise RuntimeError('Importer failed; partial database is not a reference')
    report=json.loads((stage/'report.json').read_text())
    assert report['complete'] and report['source_sha256']==manifest['input']['sha256']
    assert report['unique_triples']==manifest['expected_graph_count']
    stat=Path(manifest['input']['source']).stat()
    assert {'size':stat.st_size,'mtime_ns':stat.st_mtime_ns}==manifest['source_stat']
    for rel,digest in manifest['source_hashes'].items():
        assert hashlib.sha256((stage/rel).read_bytes()).hexdigest()==digest
    result.update(complete=True,unique_triples=report['unique_triples'],
                  graph_count_matches_independent_jena=True,source_and_parser_identity_pass=True)
except BaseException as exc:
    result['error']=type(exc).__name__+': '+str(exc)
finally:
    result['end_unix']=time.time()
    (stage/'completion.json').write_text(json.dumps(result,indent=2))
    if (lock/'owner').read_text().strip()==owner:
        (lock/'owner').unlink();lock.rmdir()
    else:raise RuntimeError('Refusing to release a different owner lock')
if not result['complete']:sys.exit(1)
