import json,subprocess,time
from pathlib import Path
p=Path(__file__).resolve().parent;root=p.parent;owner='Jianlin task25 aggregate72 matched VM disk v1'
command=['python3',str(root/'aggregate72-preparation-v1/source/supervise_import.py'),'--root',str(root),'--receipt',str(p/'run-supervisor.json'),'--owner',owner,'--','python3',str(p/'run.py')]
result=subprocess.run(command)
probe=subprocess.run(['docker','ps','-a','--filter','name=rdf-jianlin-agg72-match-','--format','{{.Names}}'],capture_output=True,text=True)
record={'supervisor_exit':result.returncode,'owned_container_probe_exit':probe.returncode,'owned_containers_remaining':probe.stdout.strip().splitlines(),'ended':time.time()}
(p/'launcher-status.json').write_text(json.dumps(record,indent=2))
if probe.returncode==0 and not probe.stdout.strip():
 lock=Path('/tmp/rdf-bench.lock/owner')
 if lock.exists() and lock.read_text().strip()==owner:lock.unlink();lock.parent.rmdir()
 message='@Kaiming-He @Alex-Chi Machine free: the aggregate72 same-VM comparison attempt has ended; owned containers are removed and the exact lock is released. I am replaying the frozen gates before publishing any totals.\n'
else:message='@Kaiming-He @Alex-Chi The aggregate72 attempt ended but owned-container cleanup could not be verified; my lock remains held for cleanup audit. No machine-free claim yet.\n'
subprocess.run(['rtk','proxy','raft','task','reply','(internal discussion)'],input=message,text=True)
