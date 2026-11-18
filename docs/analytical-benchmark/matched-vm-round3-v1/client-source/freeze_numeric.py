#!/usr/bin/env python3
"""Freeze a verified numeric-input bundle and tested validator before holdout runs."""
import argparse
from datetime import datetime,timezone
import hashlib
import json
from pathlib import Path

SOURCES=['float_envelope.py','envelope_qualify.py','qualify.py','numeric_inputs.py',
         'workload_check.py','bi_sql.py','fixture_check.py','freeze_numeric.py',
         'test_float_envelope.py','test_envelope_qualify.py','test_qualify.py']


def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()


def freeze(args):
    if Path('/tmp/rdf-bench.lock/owner').read_text().strip()!=args.lock_owner:
        raise RuntimeError('Own the lock for the final validator/input verification')
    target=args.inputs/'freeze.json'
    if target.exists():raise FileExistsError('A freeze is immutable; create another named bundle')
    verified=json.loads((args.inputs/'count-verification.json').read_text())
    if not verified['all_counts_agree'] or verified['plan_sha256']!=digest(args.inputs/'plan.json'):
        raise ValueError('Independent group counts are not verified for this plan')
    tests=json.loads(args.tests_report.read_text())
    if tests.get('exit_code')!=0:raise ValueError('Validator tests did not pass')
    source_dir=Path(__file__).parent
    sources={name:digest(source_dir/name) for name in SOURCES}
    if tests['sources']!=sources:raise ValueError('Validator changed after its tests')
    (args.inputs/'validator-tests.json').write_text(json.dumps(tests,indent=2))
    files={str(path.relative_to(args.inputs)):digest(path) for path in args.inputs.rglob('*')
           if path.is_file() and path.name!='freeze.json'}
    record={'schema':'bi-numeric-freeze-v1','state':'frozen','cohort':args.cohort,
            'created_utc':datetime.now(timezone.utc).isoformat(),'validator_sources':sources,
            'input_files':files,'previously_examined_mixes':[0] if args.cohort=='10m-confirmatory' else [],
            'unseen_confirmatory_mixes':[1,2,3] if args.cohort=='10m-confirmatory' else [],
            'prior_four_ulp_results_reclassified':False,
            'scope':'Exact-anchor float SUM/AVG outputs only; identities/counts/types/membership/order remain strict'}
    target.write_text(json.dumps(record,indent=2))
    print(json.dumps({'freeze_sha256':digest(target),'file':str(target),'cohort':args.cohort},indent=2))


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--inputs',type=Path,required=True);p.add_argument('--tests-report',type=Path,required=True)
    p.add_argument('--lock-owner',required=True);p.add_argument('--cohort',choices=['smoke','10m-confirmatory'],required=True)
    freeze(p.parse_args())
