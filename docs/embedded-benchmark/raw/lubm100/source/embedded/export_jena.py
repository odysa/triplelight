#!/usr/bin/env python3
"""Untimed independent Jena TDB2 exports, using CLI rather than HTTP."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

from contract import json_term, signature, check_order
from run import require_lock


def export(args):
    require_lock(args)
    args.out.mkdir(exist_ok=False, parents=True)
    refs = {}
    specs = json.loads(args.order_spec.read_text()) if args.order_spec else {}
    provenance = {"jena_source": args.jena_source, "store": str(args.store.resolve()), "queries": {}}
    for query in sorted(args.queries.glob('*.rq')):
        require_lock(args)
        rawpath = args.out/(query.stem+'.json')
        command = [str(args.jena/'bin/tdb2.tdbquery'), '--loc='+str(args.store),
                   '--query='+str(query.resolve()), '--results=JSON']
        with rawpath.open('wb') as stream, (args.out/(query.stem+'.stderr')).open('wb') as err:
            subprocess.run(command, stdout=stream, stderr=err, check=True,
                           env={**os.environ, 'JVM_ARGS':'-Xmx8g'})
        raw = json.loads(rawpath.read_text())
        variables = raw['head']['vars']
        rows = [tuple(json_term(binding.get(v)) for v in variables) for binding in raw['results']['bindings']]
        refs[query.stem] = signature(variables, rows)
        order = check_order(variables, rows, specs[query.stem]) if query.stem in specs else None
        provenance['queries'][query.stem] = {'command':command,
                'query_sha256':hashlib.sha256(query.read_bytes()).hexdigest(),
                'json_sha256':hashlib.sha256(rawpath.read_bytes()).hexdigest(), 'order':order}
        with rawpath.open('rb') as source, gzip.open(str(rawpath)+'.gz','wb') as target:
            shutil.copyfileobj(source,target)
        rawpath.unlink()
        (args.out/'reference.json').write_text(json.dumps(refs,indent=2))
        (args.out/'provenance.json').write_text(json.dumps(provenance,indent=2))
        print(query.stem, refs[query.stem]['rows'], flush=True)
        del raw, rows


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    for name in ('jena','store','queries','out'):
        p.add_argument('--'+name,type=Path,required=True)
    p.add_argument('--jena-source',required=True)
    p.add_argument('--lock-owner',required=True)
    p.add_argument('--order-spec',type=Path)
    export(p.parse_args())
