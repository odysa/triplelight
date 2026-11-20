#!/usr/bin/env python3
"""Verify archived tiny observations without importing or executing an engine."""
from collections import Counter
import hashlib
import json
from pathlib import Path
import tarfile
import tempfile

PACKET_SHA = '8fbd7f454754238407aeb266339d5545e254443f8f1ce454850b1420a0e58092'


def sha(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()


def term_key(t):
    if t['type'] == 'uri':
        return ('uri', t['value'], '', '')
    lang = t.get('xml:lang', '').lower()
    dt = t.get('datatype') or ('http://www.w3.org/1999/02/22-rdf-syntax-ns#langString' if lang else 'http://www.w3.org/2001/XMLSchema#string')
    return ('literal', t['value'], dt, lang)


def main():
    here = Path(__file__).resolve().parent
    assert sha(here / 'packet.tar.gz') == PACKET_SHA
    with tempfile.TemporaryDirectory(prefix='r2rml46-replay-') as temp:
        root = Path(temp)
        with tarfile.open(here / 'packet.tar.gz') as t:
            t.extractall(root, filter='data')
        manifest = json.loads((root / 'PACKET_FILES.json').read_text())
        for name, digest in manifest.items():
            assert sha(root / name) == digest, name
        for source in ['source', 'source-v1']:
            freeze = json.loads((root / source / 'FREEZE.json').read_text())
            for name, digest in freeze['files'].items():
                assert sha(root / source / name) == digest, (source, name)
        v1 = json.loads((root / 'raw/v1/supervisor.json').read_text())
        assert v1['exit_code'] == 1 and v1['child_exited'] and not v1['timed_out']
        assert not (root / 'raw/v1/results/summary.json').exists()
        assert '_sqlite3' in (root / 'raw/v1/stderr.log').read_text()
        v2 = json.loads((root / 'raw/v2/supervisor.json').read_text())
        assert v2['exit_code'] == 0 and v2['child_exited'] and not v2['timed_out']
        post = json.loads((root / 'raw/v2/results/postflight.json').read_text())
        assert all(type(v) is bool and v for v in post.values())
        summary = json.loads((root / 'raw/v2/results/summary.json').read_text())
        records = summary['records']
        expected_ids = [c['id'] for c in json.loads((root / 'source/inputs.json').read_text())['cases']]
        assert len(expected_ids) == 13 and len(set(expected_ids)) == 13 and len(records) == 26
        for engine in ['duckdb', 'sqlite']:
            assert Counter(r['id'] for r in records if r['engine'] == engine) == Counter(expected_ids)
        assert summary['admitted_duckdb'] is False and summary['admitted_sqlite'] is False
        assert summary['full_workload_qualified'] is False and summary['scored'] is False
        data = {(r['id'], r['engine']): r for r in records}
        for (name, engine), r in data.items():
            saved = json.loads((root / f'raw/v2/results/{name}-{engine}.json').read_text())
            assert saved == r
            translated = json.loads((root / f'raw/v2/results/{name}-translation.json').read_text())
            assert translated['sql'] == r['sql']
            assert all(type(v) is bool for v in r.get('checks', {}).values())
        reference = json.loads((root / 'source/consumer-fixture-jena.json').read_text())
        expected = Counter(term_key(b['o']) for b in reference['results']['bindings'])
        assert sum(expected.values()) == 9 and all(int(b['n']['value']) == 1 for b in reference['results']['bindings'])
        typed = data['typed-projection', 'duckdb']
        actual = Counter(('uri' if r[1] else 'literal', r[0], r[3] or '', r[4] or '') for r in typed['rows'])
        assert actual == expected and typed['checks']['full_rdf_term_projection'] is True
        # Extra discriminator columns are usable metadata, not a semantic loss.
        distinct = data['distinct-nine', 'duckdb']
        assert distinct['columns'] == ['v_o', 'd_o']
        decoded = []
        for value, tag in distinct['rows']:
            if tag == 'I':
                decoded.append(('uri', value, '', ''))
            elif tag.startswith('@'):
                decoded.append(('literal', value, 'http://www.w3.org/1999/02/22-rdf-syntax-ns#langString', tag[1:].lower()))
            else:
                assert tag.startswith('D')
                decoded.append(('literal', value, tag[1:], ''))
        assert Counter(decoded) == expected
        assert distinct['checks']['expected_scalar_bag'] is False  # Original adapter-shape verdict remains.
        plain = data['project-nine', 'duckdb']
        assert plain['columns'] == ['v_o'] and len(plain['rows']) == 8
        assert plain['checks']['row_count'] is False
        for name in ['count-nine', 'distinct-count-nine']:
            assert data[name, 'duckdb']['rows'] == [['8']]
            assert data[name, 'duckdb']['checks']['expected_scalar_bag'] is False
        sqlite_errors = [r for r in records if r['engine'] == 'sqlite' and 'error' in r]
        assert len(sqlite_errors) == 10
        sqlite_pass = [r['id'] for r in records if r['engine'] == 'sqlite' and 'error' not in r]
        assert set(sqlite_pass) == {'duplicate-input', 'employee-null', 'empty-projection'}
        assert all(all(data[name, 'sqlite']['checks'].values()) for name in sqlite_pass)
        anomaly = json.loads((root / 'raw/lock-anomaly.json').read_text())
        assert anomaly['self_release_verified'] is False and anomaly['own_child_exited'] is True
        result = {'packet_files_verified': len(manifest), 'query_cases': 13, 'engine_observations': 26,
                  'duckdb_plain_rows': 8, 'expected_plain_rows': 9, 'duckdb_both_counts': 8,
                  'duckdb_typed_projection_matches_nine_terms': True,
                  'duckdb_distinct_discriminator_matches_nine_terms': True,
                  'distinct_original_failure_is_adapter_shape': True,
                  'sqlite_execution_errors': 10, 'sqlite_passing_controls': sorted(sqlite_pass),
                  'admitted_duckdb': False, 'admitted_sqlite': False,
                  'lock_release_anomaly_retained': True, 'engine_executed_by_replay': False}
        print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
