#!/usr/bin/env python3
"""Prepare new 100M exact-input diagnostics without editing the frozen 10M tools.

This performs no SQL, SPARQL or candidate-output reads. It does not freeze or
qualify anything: independent input collection and Jena counts are still needed.
"""
import argparse
import hashlib
import json
from pathlib import Path
from types import SimpleNamespace

from numeric_inputs import make


def prepare(workload, out):
    manifest = json.loads((workload/'manifest.json').read_text())
    if manifest['scale'] != 'bsbm100' or manifest['mixes'] != 4 or manifest['warmup_mixes'] != 1:
        raise ValueError('This preparation is scoped to the preregistered 100M four-mix workload')
    make(SimpleNamespace(workload=workload, out=out))
    path = out/'plan.json'
    plan = json.loads(path.read_text())
    # The original collector's make() embeds the old 10M mix-0 observation.
    # Here all 100M BI outputs are unseen. This is a new, unfrozen bundle;
    # neither the old collector nor any existing freeze is changed.
    plan['previously_examined_mixes'] = []
    plan['unseen_output_mixes'] = [0, 1, 2, 3]
    plan['warmup_mixes'] = [0]
    plan['unseen_confirmatory_mixes'] = [1, 2, 3]
    plan['cohort'] = '100m-input-preparation-v1'
    plan['validation_state'] = 'inputs-not-collected-or-independently-verified'
    plan['preparation_source_sha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    path.write_text(json.dumps(plan, indent=2))
    return plan


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--workload', type=Path, required=True)
    p.add_argument('--out', type=Path, required=True)
    args = p.parse_args()
    plan = prepare(args.workload, args.out)
    print(json.dumps({'cases': len(plan['cases']), 'instances': len(plan['instances']),
                      'validator_frozen': plan['validator_frozen'],
                      'candidate_outputs_read': False}))
