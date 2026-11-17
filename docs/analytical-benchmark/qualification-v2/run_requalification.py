from pathlib import Path
import hashlib,json,os,subprocess,sys
root=Path(__file__).resolve().parent
python=Path('<agent-workspace>/scratch/analytical25-venv/bin/python')
owner='Jianlin-Su task25 float requalification'
base=root/'requalification-source/analytical'
env={**os.environ,'RAYON_NUM_THREADS':'4'};env.pop('PYTHONPATH',None)
summary={}
for name,templates in [('fixture-float',[1,2,3,4,5,6,7,8]),('fixture-cutoff',[7]),('fixture-invalid-price',[4,5,8]),('fixture-invalid-rating',[6])]:
    if Path('/tmp/rdf-bench.lock/owner').read_text().strip()!=owner:raise RuntimeError('lock changed')
    fixture=root/name
    with (fixture/'run.log').open('wb') as log:
        if not (fixture/'duck.db').exists():
            digest=hashlib.sha256((fixture/'data.nt').read_bytes()).hexdigest()
            subprocess.run([str(python),str(base/'duck_load.py'),'--source',str(fixture/'data.nt'),'--database',str(fixture/'duck.db'),'--spill',str(fixture/'spill'),'--report',str(fixture/'duck-import.json'),'--expected-sha256',digest,'--memory-limit','256MB','--max-spill','256MB','--threads','1','--lock-owner',owner],env=env,stdout=log,stderr=subprocess.STDOUT,check=True)
        subprocess.run([str(python),str(base/'fixture_check.py'),'--fixture',str(fixture),'--jena','<agent-workspace>/rdfdb/bench/tools/apache-jena-6.2.0','--rdfdb-source','63d54f75168ebac7cc4c52e536fcfc00809cc53c rebuilt isolated binding','--lock-owner',owner,'--templates',*[str(n) for n in templates]],env=env,stdout=log,stderr=subprocess.STDOUT,check=True)
    matrix=json.loads((fixture/'qualification/matrix.json').read_text())
    summary[name]={q:{s:v[s]['status'] for s in ['duckdb','rdfdb']} for q,v in matrix['queries'].items()}
    (root/'requalification-summary.json').write_text(json.dumps(summary,indent=2));print(name,summary[name],flush=True)
print('ALL REQUESTED FIXTURES FINISHED',flush=True)
