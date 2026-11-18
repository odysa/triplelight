import subprocess,json,time,hashlib,shutil
from pathlib import Path
p=Path(__file__).resolve().parent;root=p.parent
cmd=['cargo','build','--locked','--release','--bin','rdfdb','-j1','--target-dir',str(root/'target')]
with (p/'build.log').open('xb') as log:
 proc=subprocess.Popen(cmd,cwd=p/'source',stdout=log,stderr=subprocess.STDOUT)
 (p/'build-process.json').write_text(json.dumps({'pid':proc.pid,'command':cmd,'cwd':str(p/'source')}))
 rc=proc.wait()
(p/'build-status.json').write_text(json.dumps({'exit_code':rc,'ended':time.time()}))
if rc==0:
 shutil.copy2(root/'target/release/rdfdb',p/'rdfdb-af856dc')
 (p/'binary-sha256.txt').write_text(hashlib.sha256((p/'rdfdb-af856dc').read_bytes()).hexdigest())
raise SystemExit(rc)
