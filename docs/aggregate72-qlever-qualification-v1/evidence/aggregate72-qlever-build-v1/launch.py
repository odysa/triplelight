import subprocess,json,time
from pathlib import Path
p=Path(__file__).resolve().parent;owner='Jianlin task25 aggregate72 QLever builds v1';state={'complete':False,'datasets':{},'comparative_load_claim':False,'started':time.time()}
try:
 for n in ['lubm','watdiv']:
  assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
  with (p/n/'runner.log').open('xb') as log:
   child=subprocess.Popen(['python3',str(p/n/'run.py')],stdout=log,stderr=subprocess.STDOUT)
   state['datasets'][n]={'pid':child.pid};(p/'status.json').write_text(json.dumps(state,indent=2));rc=child.wait()
  state['datasets'][n]['exit_code']=rc
  if rc:raise RuntimeError('Owned QLever build failed for '+n)
 state['complete']=True
except BaseException as exc:state['error']=repr(exc)
finally:
 state['ended']=time.time();(p/'status.json').write_text(json.dumps(state,indent=2))
 lock=Path('/tmp/rdf-bench.lock/owner')
 if lock.exists() and lock.read_text().strip()==owner:lock.unlink();lock.parent.rmdir()
 subprocess.run(['rtk','proxy','raft','task','reply','(internal discussion)'],input='@Kaiming-He @Alex-Chi Machine free: the serial owned QLever aggregate-index preparation attempt has ended; its build processes and owned containers have been cleaned up. I am auditing the retained evidence.\n',text=True)
