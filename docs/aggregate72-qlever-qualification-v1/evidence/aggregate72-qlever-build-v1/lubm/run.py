"""Pinned owned QLever preparation build; never a comparative load result."""
import hashlib,json,subprocess,sys,time
from pathlib import Path
stage=Path(__file__).resolve().parent;root=Path('/private/tmp/rdf-jianlin-analytical25')
owner='Jianlin task25 aggregate72 QLever builds v1'
lock=Path('/tmp/rdf-bench.lock')
source=Path('<agent-workspace>/rdfdb/bench/tools/lubm100.nt')
image='adfreiburg/qlever@sha256:d7ecda47b4d0132c190b4a73279761fd1c614f24cba4e894ca1ba8117cef6727'
container='rdf-jianlin-agg72-ql-lubm-v1'
def sha(path):
    h=hashlib.sha256()
    with path.open('rb') as stream:
        while b:=stream.read(8*1024*1024):h.update(b)
    return h.hexdigest()
def source_identity():
    before=source.stat();digest=sha(source);after=source.stat()
    assert (before.st_size,before.st_mtime_ns)==(after.st_size,after.st_mtime_ns)
    assert digest=='49eb4a916134e35a582f159e26f5abef42e6c5fe379dc64c2e84069553c38e6d'
    assert after.st_size==2377133547
    return {'sha256':digest,'bytes':after.st_size,'mtime_ns':after.st_mtime_ns}
state={'complete':False,'comparative_load_claim':False,'started_unix':time.time()}
try:
    assert (lock/'owner').read_text().strip()==owner
    state['input_before']=source_identity()
    command=['docker','run','--name',container,'--cpus=4','--memory=12g','--memory-swap=12g',
      '--mount',f'type=bind,source={source},target=/input/data.nt,readonly',
      '--mount',f'type=bind,source={stage}/index,target=/index','--workdir','/index',
      '--env','TMPDIR=/index/tmp','--entrypoint','/qlever/qlever-index',image,
      '-i','lubm100','-f','/input/data.nt','-F','nt','-p','true','-j','4','-m','4GB']
    guarded=[sys.executable,str(stage/'supervise_import.py'),'--root',str(root),
      '--receipt',str(stage/'supervisor.json'),'--owner',owner,'--',*command]
    (stage/'command.json').write_text(json.dumps({'command':command,'guarded_command':guarded,
      'sort_memory_setting':'4GB; advisory indexing budget, not RSS limit',
      'container_memory_bytes':12*1024**3,'container_swap_total_bytes':12*1024**3,
      'cpus':4,'index_threads':4,'parallel_parser':True,'all_six_permutations':True},indent=2))
    with (stage/'build.log').open('wb') as log:
        result=subprocess.run(guarded,stdout=log,stderr=subprocess.STDOUT)
    state['guard_exit']=result.returncode
    if result.returncode:raise RuntimeError('Index build failed; retain incomplete output')
    state['input_after']=source_identity();assert state['input_before']==state['input_after']
    meta=stage/'index/lubm100.meta-data.json'
    state['index_files']={str(p.relative_to(stage/'index')):{'bytes':p.stat().st_size,'sha256':sha(p)}
      for p in sorted((stage/'index').rglob('*')) if p.is_file()}
    if not meta.is_file():raise RuntimeError('No completed metadata')
    state['metadata']=json.loads(meta.read_text())
    state['complete']=True
except BaseException as exc:state['error']=type(exc).__name__+': '+str(exc)
finally:
    inspect=subprocess.run(['docker','inspect',container,'--format','{{json .State}}'],capture_output=True,text=True)
    state['container_state_before_cleanup']=inspect.stdout.strip()
    if inspect.returncode==0:
        value=json.loads(inspect.stdout)
        if value.get('Running'):
            subprocess.run(['docker','stop','--time','15',container],capture_output=True,check=True)
        subprocess.run(['docker','rm',container],capture_output=True,check=True)
    state['ended_unix']=time.time();(stage/'status.json').write_text(json.dumps(state,indent=2))
sys.exit(0 if state['complete'] else 1)
