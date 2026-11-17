from pathlib import Path
import hashlib,json,os,subprocess,sys
root=Path(__file__).resolve().parent
run=root/'qualification-v3-run'
python=Path('<agent-workspace>/scratch/analytical25-venv/bin/python')
owner='Jianlin-Su task25 aggregate-error requalification'
base=run/'source/analytical'
jena='<agent-workspace>/rdfdb/bench/tools/apache-jena-6.2.0'
commit='d2618c6c9c6d51541d9298e8db88f131f5ab2eb6'
env={**os.environ,'RAYON_NUM_THREADS':'4'};env.pop('PYTHONPATH',None)
summary={}
def call(args,log):
    if Path('/tmp/rdf-bench.lock/owner').read_text().strip()!=owner:raise RuntimeError('lock changed')
    subprocess.run([str(python),*[str(a) for a in args]],env=env,stdout=log,stderr=subprocess.STDOUT,check=True)
for name,templates in [('normal',list(range(1,9))),('cutoff',[7]),('invalid-price',[4,5,8]),('invalid-rating',[6]),('empty',list(range(1,9)))]:
    fixture=run/name
    with (run/(name+'.log')).open('wb') as log:
        if name=='empty':call([base/'make_fixture.py',fixture,'--empty'],log)
        if not (fixture/'duck.db').exists():
            digest=hashlib.sha256((fixture/'data.nt').read_bytes()).hexdigest()
            call([base/'duck_load.py','--source',fixture/'data.nt','--database',fixture/'duck.db','--spill',fixture/'spill','--report',fixture/'duck-import.json','--expected-sha256',digest,'--memory-limit','256MB','--max-spill','256MB','--threads','1','--lock-owner',owner],log)
        call([base/'fixture_check.py','--fixture',fixture,'--jena',jena,'--rdfdb-source',commit+' rebuilt isolated binding','--lock-owner',owner,'--templates',*map(str,templates)],log)
    matrix=json.loads((fixture/'qualification/matrix.json').read_text())
    summary[name]={q:{s:v[s]['status'] for s in ['duckdb','rdfdb']} for q,v in matrix['queries'].items()}
    (run/'summary.json').write_text(json.dumps(summary,indent=2));print(name,summary[name],flush=True)
with (run/'aggregate-diagnostic.log').open('wb') as log:
    fixture=run/'aggregate-diagnostic'
    call([base/'aggregate_check.py','--fixture',fixture,'--make'],log)
    call([base/'aggregate_check.py','--fixture',fixture,'--jena',jena,'--rdfdb-source',commit+' rebuilt isolated binding','--lock-owner',owner],log)
matrix=json.loads((fixture/'qualification/matrix.json').read_text())
summary['aggregate-diagnostic']={q:v['status'] for q,v in matrix['cases'].items()}
(run/'summary.json').write_text(json.dumps(summary,indent=2))
print('aggregate-diagnostic',summary['aggregate-diagnostic'],flush=True)
print('ALL REQUESTED FIXTURES FINISHED',flush=True)
