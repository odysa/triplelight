#!/usr/bin/env python3
"""Summarize the two serial, reversed-order passes; reject incomplete evidence."""
import argparse
import json
import math
from pathlib import Path
import re
import statistics


def summarize(folder, only_dataset=None):
    groups = {}
    sequence_path = folder/'sequence.json' if (folder/'sequence.json').is_file() else folder.parent/'sequence.json'
    sequence = json.loads(sequence_path.read_text())
    for path in sorted(folder.glob('*.json')):
        match = re.fullmatch(r'(lubm100|bsbm10)-p([12])-(.+)', path.stem)
        if not match:
            continue
        dataset, passno, system = match.groups()
        if only_dataset is not None and dataset != only_dataset:
            continue
        if sequence.get(path.stem, {}).get('exit_code') != 0:
            raise ValueError('No successful process exit recorded: '+str(path))
        run = json.loads(path.read_text())
        if not run.get('complete') or not run.get('externally_validated') or run.get('error'):
            raise ValueError('Incomplete or unvalidated run: '+str(path))
        groups.setdefault(dataset, {}).setdefault(system, {})[passno] = (path, run)
    summary = {}
    for dataset, systems in groups.items():
        summary[dataset] = {}
        for system, passes in systems.items():
            if set(passes) != {'1', '2'}:
                raise ValueError('Need both process orders: '+system)
            runs = [passes[k][1] for k in ('1', '2')]
            if set(runs[0]['queries']) != set(runs[1]['queries']):
                raise ValueError('Query cohort changed')
            if runs[0]['binding_source'] != runs[1]['binding_source']:
                raise ValueError('Binding source changed')
            queries = {}
            for name in runs[0]['queries']:
                raw = [r['queries'][name] for r in runs]
                samples = [s for q in raw for s in q['samples']]
                expected = samples[0]['signature']
                if any(s['signature'] != expected or not s['matches_reference'] for s in samples):
                    raise ValueError('Answer mismatch')
                measured = [s for s in samples if not s['warmup']]
                queries[name] = {'rows': expected['rows'], 'sha256': expected['sha256'],
                                'measured_samples': len(measured),
                                'order_checked': all(s['order']['checked'] for s in samples),
                                'process_medians_ms': [q['median_fully_consumed_ms'] for q in raw],
                                **{phase: statistics.median(s[phase] for s in measured)
                                   for phase in ('fully_consumed_ms', 'execute_return_ms',
                                                 'materialize_ms', 'semantic_decode_ms')}}
            rss = []
            for path, _ in passes.values():
                log = path.with_suffix('.log').read_text()
                found = re.search(r'^\s*(\d+)\s+maximum resident set size\s*$', log, re.M)
                rss.append(int(found[1]) if found else None)
            latencies = [q['fully_consumed_ms'] for q in queries.values()]
            summary[dataset][system] = {
                'sum_query_medians_ms': sum(latencies),
                'geomean_ms': math.exp(sum(map(math.log, latencies))/len(latencies)),
                'sum_without_q6_q14_ms': sum(q['fully_consumed_ms'] for name,q in queries.items() if name not in ('q6','q14')) if dataset=='lubm100' else None,
                'process_open_ms': [r['open_ms'] for r in runs],
                'process_ready_ms': [r['ready_ms'] for r in runs],
                'process_rss_bytes_macos': rss, 'queries': queries}
        # Different representations and schemas must still match the same entire bags.
        for name in next(iter(summary[dataset].values()))['queries']:
            signatures = {(s['queries'][name]['rows'], s['queries'][name]['sha256']) for s in summary[dataset].values()}
            if len(signatures) != 1:
                raise ValueError('Cross-system answer mismatch: '+name)
    if not summary:
        raise ValueError('No completed reversed-order cohorts')
    return summary


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('folder',type=Path)
    p.add_argument('--dataset', choices=['lubm100', 'bsbm10'], help='Explicit partial-cohort summary; does not validate the excluded dataset')
    args=p.parse_args()
    print(json.dumps(summarize(args.folder,args.dataset),indent=2))
