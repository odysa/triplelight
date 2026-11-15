#!/usr/bin/env python3
"""Own the benchmark lock before running. Serial fixed binaries, four Rayon workers."""
import sys,os,subprocess
from pathlib import Path
root=Path(__file__).resolve().parents[2]
tools=Path(sys.argv[1]);binaries=Path(sys.argv[2])
steps=[]
for family,dataset in [('lubm','lubm100'),('watdiv','watdiv100')]:
    steps.append((family,dataset,'candidate-v3',root/'docs/count-pushdown/queries'/family,family+'-candidate-v3',True))
for family,dataset in [('lubm','lubm100'),('watdiv','watdiv100')]:
    for variant in ['baseline','candidate-v3']:
        steps.append((family,dataset,variant,root/'bench'/family,'original-'+family+'-'+variant,False))
for family,dataset,variant,queries,label,save in steps:
    out=root/'docs/count-pushdown/results'/(label+'.tsv')
    env=dict(os.environ,RAYON_NUM_THREADS='4')
    env.pop('COUNT_PROBE_SAVE_JSON',None)
    if save:env['COUNT_PROBE_SAVE_JSON']='1'
    with out.with_suffix('.log').open('w') as log:
        subprocess.run([str(binaries/variant),str(tools/('rdfdb-disk-'+dataset)),str(queries),str(out)],env=env,stdout=log,stderr=subprocess.STDOUT,check=True)
    print('finished',label,flush=True)
