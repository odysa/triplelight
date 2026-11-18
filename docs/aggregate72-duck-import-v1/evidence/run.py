import subprocess,json,hashlib,time
from pathlib import Path
p=Path(__file__).resolve().parent;root=p.parent
plan=json.loads((p/'planned-commands.json').read_text())
state={'complete':False,'datasets':{},'comparative_load_claim':False,'started':time.time()}
def save():(p/'status.json').write_text(json.dumps(state,indent=2))
try:
 for dataset,cmd in plan['commands'].items():
  assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==plan['owner']
  with (p/dataset/'import.log').open('xb') as log:
   proc=subprocess.Popen(cmd,stdout=log,stderr=subprocess.STDOUT)
   state['datasets'][dataset]={'pid':proc.pid};save();rc=proc.wait()
  entry=state['datasets'][dataset];entry['exit_code']=rc;save()
  if rc:raise RuntimeError('Import failed for '+dataset)
  report=json.loads((p/dataset/'report.json').read_text())
  ref=json.loads((root/'aggregate72-reference-v2'/dataset/'dataset-count.json').read_text())
  expected=int(ref['results']['bindings'][0]['n']['value'])
  assert report['complete'] and report['unique_triples']==expected
  entry.update(unique_triples=report['unique_triples'],reference_count=expected)
  h=hashlib.sha256()
  with (p/dataset/'duck.db').open('rb') as f:
   while b:=f.read(8*1024*1024):h.update(b)
  entry['database_sha256']=h.hexdigest();save()
 state['complete']=True
except BaseException as exc:state['error']=repr(exc);raise
finally:state['ended']=time.time();save()
