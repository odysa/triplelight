#!/usr/bin/env python3
"""Freeze independently verified 100M anchors before any BI result inspection.

Leaves the protected 10M freezer and arithmetic validators unchanged.
"""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
from freeze_numeric import SOURCES as ORIGINAL_SOURCES, digest

SOURCES=ORIGINAL_SOURCES+['prepare_numeric100.py','freeze_numeric100.py','test_freeze_numeric100.py']

def validate(plan,verified,identity):
    if plan['dataset_sha256']!=identity['sha256']:
        raise ValueError('Input source identity differs')
    if plan.get('previously_examined_mixes')!=[] or plan.get('collector_reads_candidate_outputs') is not False:
        raise ValueError('This freeze requires the still-unseen 100M cohort')
    if not verified['all_counts_agree'] or set(verified['cases'])!=set(plan['cases']):
        raise ValueError('Incomplete independent count coverage')
    if any(not case['counts_agree'] for case in verified['cases'].values()):
        raise ValueError('An independent count differs')
    if len(plan['cases'])!=28 or len(plan['instances'])!=32:
        raise ValueError('Unexpected registered 100M numeric cohort')

def freeze(args):
    if Path('/tmp/rdf-bench.lock/owner').read_text().strip()!=args.lock_owner:
        raise RuntimeError('Own the lock for final source and validator verification')
    target=args.inputs/'freeze.json'
    if target.exists():raise FileExistsError('An input freeze is immutable')
    plan=json.loads((args.inputs/'plan.json').read_text())
    verified=json.loads((args.inputs/'count-verification.json').read_text())
    identity=json.loads((args.inputs/'input-identity.json').read_text())
    validate(plan,verified,identity)
    if verified['plan_sha256']!=digest(args.inputs/'plan.json'):
        raise ValueError('Independent counts belong to a different plan')
    for key,case in plan['cases'].items():
        if not (args.inputs/'sql'/(key+'.json')).is_file():raise ValueError('Missing SQL anchors')
        for query in case['jena'].values():
            if digest(args.inputs/query['file'])!=query['sha256']:
                raise ValueError('Independent diagnostic query changed')
    source_dir=Path(__file__).parent
    sources={name:digest(source_dir/name) for name in SOURCES}
    tests=json.loads(args.tests_report.read_text())
    if tests.get('exit_code')!=0 or tests.get('sources')!=sources:
        raise ValueError('Validator tests/source identity differ')
    (args.inputs/'validator-tests.json').write_text(json.dumps(tests,indent=2))
    files={str(p.relative_to(args.inputs)):digest(p) for p in args.inputs.rglob('*')
           if p.is_file() and p.name!='freeze.json'}
    result={'schema':'bi-numeric-freeze-100m-v1','state':'frozen',
      'cohort':'100m-confirmatory','created_utc':datetime.now(timezone.utc).isoformat(),
      'validator_sources':sources,'input_files':files,'previously_examined_mixes':[],
      'unseen_confirmatory_mixes':[1,2,3],'unseen_warmup_mixes':[0],
      'prior_four_ulp_results_reclassified':False,
      'scope':'Exact-anchor float SUM/AVG only; RDF identities/counts/types/membership/order stay strict'}
    target.write_text(json.dumps(result,indent=2))
    print(json.dumps({'freeze_sha256':digest(target),'cohort':result['cohort']}))

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--inputs',type=Path,required=True)
    parser.add_argument('--tests-report',type=Path,required=True)
    parser.add_argument('--lock-owner',required=True)
    freeze(parser.parse_args())
