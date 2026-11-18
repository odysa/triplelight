import argparse,hashlib,json,shutil,subprocess,sys,time
from pathlib import Path
from types import SimpleNamespace
stage=Path(__file__).resolve().parent
owner='Jianlin-Su task25 100M exact numeric input collection'
sys.path.insert(0,str(stage/'collector-source'))
import numeric_inputs
plan=json.loads((stage/'plan.json').read_text())
parser=argparse.ArgumentParser();parser.add_argument('--case');args=parser.parse_args()
if args.case:
    case=plan['cases'][args.case]
    out=stage/'case-work'/args.case
    out.mkdir(parents=True)
    shutil.copytree(stage/args.case,out/args.case)
    settings=SimpleNamespace(out=out,lock_owner=owner,threads=4,
        database=stage.parent/'duck100-import-v2/duck.db',
        import_report=stage.parent/'duck100-import-v2/report.json')
    numeric_inputs.collect_sql(settings,dict(plan,cases={args.case:case}))
else:
    dest=stage/'sql';dest.mkdir(exist_ok=True)
    status={'complete':False,'cases':{},'start_unix':time.time()}
    try:
        for key in plan['cases']:
            target=dest/(key+'.json')
            if target.exists():
                status['cases'][key]={'already_collected':True};continue
            with (stage/(key+'.log')).open('x') as log:
                child=subprocess.Popen([sys.executable,str(Path(__file__)), '--case',key],stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
                try:
                    from supervise_import import snapshot
                    deadline=time.monotonic()+900
                    while child.poll() is None:
                        point=snapshot(stage.parent,owner)
                        with (stage/'guard-samples.jsonl').open('a') as samples:samples.write(json.dumps(point)+'\n')
                        if point['abort']:raise RuntimeError(point['abort'])
                        if time.monotonic()>deadline:raise TimeoutError('900-second per-case limit')
                        try:child.wait(timeout=2)
                        except subprocess.TimeoutExpired:pass
                    code=child.returncode
                except BaseException:
                    import os,signal
                    if child.poll() is None:
                        os.killpg(child.pid,signal.SIGTERM)
                        try:child.wait(timeout=15)
                        except subprocess.TimeoutExpired:os.killpg(child.pid,signal.SIGKILL);child.wait()
                    raise
            status['cases'][key]={'exit_code':code}
            if code:raise RuntimeError('Diagnostic collection failed: '+key)
            source=stage/'case-work'/key/'sql'/(key+'.json')
            shutil.copy2(source,target)
            status['cases'][key]['sha256']=hashlib.sha256(target.read_bytes()).hexdigest()
            (stage/'sql-status.json').write_text(json.dumps(status,indent=2))
        status['complete']=True
    finally:
        status['end_unix']=time.time();(stage/'sql-status.json').write_text(json.dumps(status,indent=2))
        lock=Path('/tmp/rdf-bench.lock')
        if (lock/'owner').read_text().strip()==owner:
            (lock/'owner').unlink();lock.rmdir()
