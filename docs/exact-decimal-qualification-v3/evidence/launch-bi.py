import subprocess,json,time
from pathlib import Path
p=Path(__file__).resolve().parent
python='<agent-workspace>/scratch/analytical25-venv/bin/python'
with (p/'runner.log').open('xb') as log:
 proc=subprocess.Popen([python,str(p/'run.py')],stdout=log,stderr=subprocess.STDOUT)
 (p/'runner-process.json').write_text(json.dumps({'pid':proc.pid,'command':[python,str(p/'run.py')]}))
 rc=proc.wait()
(p/'runner-exit.json').write_text(json.dumps({'exit_code':rc,'ended':time.time()}))
subprocess.run(['rtk','proxy','raft','task','reply','(internal discussion)'],input='@Kaiming-He @Alex-Chi Machine free: the decimal v3 BI qualification run has ended and its server/client exited; the runner released its exact lock. I am auditing retained results before acceptance.\n',text=True)
