from pathlib import Path
import json,os,subprocess,sys,time
root=Path(__file__).resolve().parent
python=Path('<agent-workspace>/scratch/ladybug18-venv/bin/python')
old=Path('/private/tmp/rdf-jianlin-ladybug18')
owner='Jianlin-Su task22 embedded comparison'
env={**os.environ,'PYTHONPATH':str(root/'python'),'RAYON_NUM_THREADS':'4'}
orders={};(root/'results').mkdir()
for dataset,query in [('lubm100','lubm'),('bsbm10','bsbm')]:
    systems=['rdfdb-objects','rdfdb-json','ladybug-generic']
    if dataset=='lubm100':systems.append('ladybug-tuned')
    for passno,order in enumerate([systems,list(reversed(systems))],1):
        for system in order:
            if Path('/tmp/rdf-bench.lock/owner').read_text().strip()!=owner:raise RuntimeError('lock changed')
            stem=f'{dataset}-p{passno}-{system}'
            native=system.startswith('ladybug')
            compiled=root/('lubm100-tuned-compiled.json' if system=='ladybug-tuned' else 'lubm100-compiled.json' if dataset=='lubm100' else 'bsbm10-compiled-v2.json')
            database=old/('lubm100-tuned.db' if system=='ladybug-tuned' else dataset+'.db') if native else root/dataset
            cmd=[str(python),str(root/'source/embedded/run.py'),'--system','ladybug' if native else 'rdfdb','--database',str(database),'--queries',str(root/'queries'/query),'--compiled',str(compiled),'--reference',str(root/(dataset+'-reference')/'reference.json'),'--binding-source','ladybug-0.22.3' if native else 'fe6db89 frozen SO d07afded246f314e5302be0b141ca90189c7c6c5df156281fae06b3c3d198477','--lock-owner',owner,'--out',str(root/'results'/(stem+'.json')),'--runs','3','--cache-mb','16384','--threads','4']
            if not native:cmd+=['--rdfdb-result',system.removeprefix('rdfdb-')]
            if query=='bsbm':cmd+=['--order-spec',str(root/'source/embedded/bsbm-order.json')]
            orders[stem]={'command':cmd,'started_unix':time.time()}
            (root/'sequence.json').write_text(json.dumps(orders,indent=2))
            print('START',stem,flush=True)
            with (root/'results'/(stem+'.log')).open('wb') as stream:
                result=subprocess.run(['/usr/bin/time','-l',*cmd],env=env,stdout=stream,stderr=subprocess.STDOUT)
            orders[stem].update(exit_code=result.returncode,ended_unix=time.time())
            (root/'sequence.json').write_text(json.dumps(orders,indent=2))
            if result.returncode:raise RuntimeError('failed '+stem)
            print('DONE',stem,flush=True)
print('ALL COHORTS COMPLETE',flush=True)
