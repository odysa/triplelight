import subprocess,json,time
from pathlib import Path
p=Path(__file__).resolve().parent;root=p.parent;owner='Jianlin task25 aggregate72 DuckDB qualification v1'
cmd=['python3',str(root/'aggregate72-preparation-v1/source/supervise_import.py'),'--root',str(root),'--receipt',str(p/'supervisor.json'),'--owner',owner,'--','python3',str(p/'run.py')]
r=subprocess.run(cmd);(p/'launcher-status.json').write_text(json.dumps({'supervisor_exit':r.returncode,'ended':time.time()}))
lock=Path('/tmp/rdf-bench.lock/owner')
if lock.exists() and lock.read_text().strip()==owner:lock.unlink();lock.parent.rmdir()
subprocess.run(['rtk','proxy','raft','task','reply','(internal discussion)'],input='@Kaiming-He @Alex-Chi Machine free: the aggregate72 DuckDB qualification attempt has ended and its supervised children exited. Results are pending audit.\n',text=True)
