import subprocess,json,time
from pathlib import Path
P=Path(__file__).resolve().parent
ROOT=P.parent
OWNER='Jianlin task25 aggregate72 Jena reference v1'
LOCK=Path('/tmp/rdf-bench.lock/owner')
cmd=['python3',str(ROOT/'aggregate72-preparation-v1/source/supervise_import.py'),'--root',str(ROOT),'--receipt',str(P/'supervisor.json'),'--owner',OWNER,'--','python3',str(P/'run.py')]
r=subprocess.run(cmd)
(P/'launcher-status.json').write_text(json.dumps({'supervisor_exit':r.returncode,'ended':time.time()}))
if LOCK.exists() and LOCK.read_text().strip()==OWNER:
 LOCK.unlink();LOCK.parent.rmdir()
subprocess.run(['rtk','proxy','raft','task','reply','(internal discussion)'],input='@Kaiming-He @Alex-Chi Machine free: my aggregate72 Jena reference attempt ended and its supervised server processes exited. I am auditing retained results; no comparative timing claim.\n',text=True)
