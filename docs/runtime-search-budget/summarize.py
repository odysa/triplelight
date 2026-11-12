"""Summarize opt-in per-request phase logs, preserving all samples.
Run from this directory after copying before.log and after.log.
"""
from pathlib import Path
import csv
import json
import re
import statistics

root = Path(__file__).resolve().parent
summary = {}
for mode in ['before', 'candidate-greedy']:
    records = []
    current = None
    for line in (root / (mode + '.log')).read_text().splitlines():
        if line.startswith('profile_begin '):
            fields = dict(re.findall(r'(\w+)=([^ ]+)', line))
            current = {'query': Path(fields['query']).stem,
                       'stats': fields['stats'], 'sample': int(fields['sample'])}
        elif line.startswith('planning_profile '):
            current.update({k: int(v) for k, v in re.findall(r'(\w+)=(\d+)', line)})
        elif line.startswith('profile_end '):
            current.update({k: int(v) for k, v in re.findall(r'(\w+)=(\d+)', line)})
            current['outside_plan_ns'] = current['query_ns'] - current.get('total_ns', 0)
            assert current['outside_plan_ns'] >= 0
            records.append(current)
            current = None
    assert len(records) in (20, 30)
    for query in {r['query'] for r in records}:
        assert len({r['rows'] for r in records if r['query'] == query}) == 1
    with (root / (mode + '.csv')).open('w', newline='') as f:
        writer = csv.DictWriter(f, fieldnames=['query', 'stats', 'sample', 'clone_ns',
            'prefix_ns', 'search_ns', 'gate_ns', 'total_ns', 'query_ns', 'outside_plan_ns', 'rows'])
        writer.writeheader()
        writer.writerows(records)
    groups = {}
    for q in sorted({r['query'] for r in records}):
        groups[q] = {}
        for flag in ['false', 'true']:
            rows = [r for r in records if r['query'] == q and r['stats'] == flag]
            groups[q][flag] = {key: statistics.median(r.get(key, 0) for r in rows)
                for key in ['clone_ns', 'prefix_ns', 'search_ns', 'gate_ns', 'total_ns', 'query_ns', 'outside_plan_ns']}
    summary[mode] = groups
(root / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
print(json.dumps(summary, indent=2))
