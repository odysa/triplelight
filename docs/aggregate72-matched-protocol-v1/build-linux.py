import subprocess,json,time,hashlib
from pathlib import Path
p=Path(__file__).resolve().parent;root=p.parent;owner='Jianlin task25 aggregate72 pinned Linux build v1';container='rdf-jianlin-agg72-linux-build-9a2f49b'
state={'complete':False,'started':time.time(),'comparative_timing_claim':False}
try:
 assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
 subprocess.run(['cp','-cR',str(root/'matched-vm-100m-v1/target-linux-ef7a778'),str(p/'target-linux-9a2f49b')],check=True)
 cmd=['docker','run','--rm','--name',container,'--cpus=4','--memory=8g','--memory-swap=8g','--mount','type=bind,source='+str(p)+',target=/work','--env','CARGO_BUILD_JOBS=2','--env','CARGO_TARGET_DIR=/work/target-linux-9a2f49b','--entrypoint','sh','rust@sha256:0b0ea86881537b3f645aace7e8e39556289f8787db057317c8d4978548213b9e','-c','mkdir /b && cp -r /work/rdfdb-source-9a2f49b/src /work/rdfdb-source-9a2f49b/Cargo.toml /work/rdfdb-source-9a2f49b/Cargo.lock /b/ && cd /b && sha256sum -c /work/rdfdb-source-9a2f49b.sha256 && cargo build --locked --release --bin rdfdb -j2']
 (p/'linux-build-command.json').write_text(json.dumps(cmd,indent=2))
 guarded=['python3',str(root/'aggregate72-preparation-v1/source/supervise_import.py'),'--root',str(root),'--receipt',str(p/'linux-build-supervisor.json'),'--owner',owner,'--',*cmd]
 with (p/'linux-build.log').open('xb') as log:proc=subprocess.Popen(guarded,stdout=log,stderr=subprocess.STDOUT);state['supervisor_pid']=proc.pid;(p/'linux-build-status.json').write_text(json.dumps(state,indent=2));rc=proc.wait()
 state['exit_code']=rc
 if rc:raise RuntimeError('Guarded Linux build failed')
 binary=p/'target-linux-9a2f49b/release/rdfdb';raw=binary.read_bytes();assert raw[:4]==b'\x7fELF'
 identity={'sha256':hashlib.sha256(raw).hexdigest(),'bytes':len(raw),'file':subprocess.check_output(['file',str(binary)],text=True),'commit':'9a2f49b652a73466873cacd0a9205098e27d15f6'}
 (p/'rdfdb-linux-binary.json').write_text(json.dumps(identity,indent=2))
 state['complete']=True
except BaseException as exc:state['error']=repr(exc)
finally:
 inspect=subprocess.run(['docker','inspect',container,'--format','{{json .State}}'],capture_output=True,text=True)
 if inspect.returncode==0:
  value=json.loads(inspect.stdout)
  if value['Running']:subprocess.run(['docker','stop','--time','10',container],capture_output=True,check=True)
 state['ended']=time.time();(p/'linux-build-status.json').write_text(json.dumps(state,indent=2))
 lock=Path('/tmp/rdf-bench.lock/owner')
 if lock.exists() and lock.read_text().strip()==owner:lock.unlink();lock.parent.rmdir()
 subprocess.run(['rtk','proxy','raft','task','reply','(internal discussion)'],input='@Kaiming-He @Alex-Chi Machine free: the guarded aggregate72 Linux build attempt has ended and its owned build container is gone/stopped. I am auditing the binary/source identity before timing registration.\n',text=True)
