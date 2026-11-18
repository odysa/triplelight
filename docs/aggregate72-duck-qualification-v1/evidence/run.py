import subprocess,json,time
from pathlib import Path
p=Path(__file__).resolve().parent;root=p.parent;f=json.loads((root/'aggregate72-preparation-v1/rules-freeze.json').read_text())
python='<agent-workspace>/scratch/analytical25-venv/bin/python'
s={'complete':False,'queries':{},'comparative_timing_claim':False,'started':time.time()}
def save():(p/'status.json').write_text(json.dumps(s,indent=2))
try:
 for name,record in f['queries'].items():
  assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()=='Jianlin task25 aggregate72 DuckDB qualification v1'
  d=p/record['dataset'];d.mkdir(exist_ok=True);stem=Path(name).stem;cmd=[python,str(p/'one.py'),name];entry={'command':cmd};s['queries'][name]=entry;save()
  with (d/(stem+'.log')).open('xb') as log:
   proc=subprocess.Popen(cmd,stdout=log,stderr=subprocess.STDOUT);entry['pid']=proc.pid;save()
   try:rc=proc.wait(timeout=600);entry['exit_code']=rc
   except subprocess.TimeoutExpired:proc.kill();proc.wait();entry['error']='600-second query cutoff';save();continue
  if rc==0:entry['qualification']=json.loads((d/(stem+'-check.json')).read_text())
  else:entry['error']='Query process failed; raw log retained'
  save()
 s['complete']=len(s['queries'])==72 and all('qualification' in v for v in s['queries'].values())
 s['primary_pass']=sum(v.get('qualification',{}).get('primary_pass',False) for v in s['queries'].values())
 s['count_compatible_pass']=sum(v.get('qualification',{}).get('count_compatible_pass',False) for v in s['queries'].values())
finally:s['ended']=time.time();save()
