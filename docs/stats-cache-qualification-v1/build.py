import hashlib,json,subprocess,time,shutil,sys
from pathlib import Path
p=Path(__file__).resolve().parent;source=p/'source';meta=json.loads((p/'source.json').read_text())
def sha(f):return hashlib.sha256(f.read_bytes()).hexdigest()
for name,h in meta['files'].items():assert sha(source/name)==h
cmd=['cargo','build','--locked','--release','--bin','triplelight','-j1','--target-dir',str(p.parent/'target')]
with (p/'build.log').open('xb') as log:
 proc=subprocess.Popen(cmd,cwd=source,stdout=log,stderr=subprocess.STDOUT);(p/'build-process.json').write_text(json.dumps({'pid':proc.pid,'command':cmd,'cwd':str(source)}));rc=proc.wait()
record={'exit_code':rc,'source_commit':meta['commit'],'ended_unix':time.time()}
for name,h in meta['files'].items():assert sha(source/name)==h
if rc==0:
 binary=p/'triplelight-c3a86d46';shutil.copyfile(p.parent/'target/release/triplelight',binary);shutil.copymode(p.parent/'target/release/triplelight',binary);record['binary_sha256']=sha(binary)
(p/'build-status.json').write_text(json.dumps(record,indent=2)+'\n');sys.exit(rc)
