#!/usr/bin/env python3
"""Serial HTTP BI measurement. Response validation is outside the request timer.

This runner does not provision servers or certify their configuration. Supply a
reviewed runtime-evidence JSON for the exact running endpoint. Preserve that
evidence with each result; a completed run is not itself a fair-comparison claim.
"""
import argparse
import hashlib
import json
from pathlib import Path
import time
import urllib.error
import urllib.parse
import urllib.request

from envelope_qualify import FrozenInputs, qualify_enveloped
from qualify import qualify, check_order, q7_allowed_products
from workload_check import diagnostics


def exchange(endpoint, fields, timeout):
    request = urllib.request.Request(endpoint,
        data=urllib.parse.urlencode(fields).encode(), headers={
            'Accept': 'application/sparql-results+json',
            'Content-Type': 'application/x-www-form-urlencoded'})
    start = time.perf_counter_ns()
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            raw = response.read()
            code = response.status
        error = None
    except urllib.error.HTTPError as exc:
        try:
            raw = exc.read()
        finally:
            exc.close()
        code, error = exc.code, 'HTTPError: ' + str(exc)
    except Exception as exc:
        # A connection/read failure has no complete body. Do not report it as a
        # successful empty answer; record elapsed time as a failed attempt only.
        raw, code, error = b'', None, type(exc).__name__ + ': ' + str(exc)
    return raw, {'elapsed_ns': time.perf_counter_ns() - start,
                 'http_status': code, 'bytes': len(raw), 'transport_error': error}


def verify(number, actual, reference, diagnostic, frozen, query):
    if number == 7:
        if actual['head']['vars'] != ['product'] or any(
                row['product']['type'] != 'uri' for row in actual['results']['bindings']):
            raise ValueError('q7 result shape/type differs')
        detail = {'q7_cutoff': q7_allowed_products(
            [r['product']['value'] for r in actual['results']['bindings']], diagnostics(diagnostic))}
    elif number in (4, 5, 6):
        detail = qualify_enveloped(number, actual, reference, frozen.for_instance(query))
    else:
        detail = qualify(number, actual, reference)
    return {'qualification': detail, 'order': check_order(number, actual),
            'rows': len(actual['results']['bindings'])}


def run(args):
    def lock():
        if Path('/tmp/rdf-bench.lock/owner').read_text().strip() != args.lock_owner:
            raise RuntimeError('Own the shared benchmark lock throughout measurement')
    lock()
    evidence_raw = args.runtime_evidence.read_bytes()
    evidence = json.loads(evidence_raw)
    # Explicit provenance is required, but the operator must independently
    # establish these statements against the actual process and image.
    for field in ('system', 'endpoint', 'source_or_image', 'dataset_sha256',
                  'threads', 'memory_configuration', 'cache_profile'):
        if field not in evidence:
            raise ValueError('Missing runtime evidence: ' + field)
    manifest_raw = (args.workload / 'manifest.json').read_bytes()
    manifest = json.loads(manifest_raw)
    if evidence['dataset_sha256'] != manifest['source_data_sha256']:
        raise ValueError('Runtime evidence belongs to another dataset')
    frozen = FrozenInputs(args.numeric_inputs, args.numeric_freeze_sha256,
                          manifest['source_data_sha256'], hashlib.sha256(manifest_raw).hexdigest())
    if evidence['system'] == 'qlever':
        if not evidence.get('cache_control_verified') or not evidence.get('cache_control_evidence'):
            raise ValueError('QLever cache profile must be independently verified first')
        if evidence['cache_profile'] == 'clear-between-requests':
            if not evidence.get('before_each_request'):
                raise ValueError('Cache-cold profile requires an explicit clear request')
        elif evidence['cache_profile'] != 'full-bypass':
            raise ValueError('Unsupported QLever cache profile')
    count_freeze = getattr(args, 'count_policy_freeze', None)
    count_policy_sha = None
    if count_freeze:
        from count_equivalence import load_freeze
        count_policy_sha = load_freeze(count_freeze, args.count_policy_sha256)
    args.out.mkdir(parents=True, exist_ok=False)
    if count_freeze:
        (args.out / 'count-policy-freeze.json').write_bytes(count_freeze.read_bytes())
    (args.out / 'runtime-evidence.json').write_bytes(evidence_raw)
    (args.out / 'manifest.json').write_bytes(manifest_raw)
    report = {'schema': 'http-bi-measurement-v1', 'complete': False,
              'runtime_evidence_sha256': hashlib.sha256(evidence_raw).hexdigest(),
              'numeric_freeze_sha256': frozen.freeze_sha256,
              'count_equivalence_freeze_sha256': count_policy_sha,
              'timer': 'HTTP request through complete body read; excludes JSON parsing and qualification',
              'queries': {}, 'ranking_eligible': False}
    def persist():
        (args.out / 'measurements.json').write_text(json.dumps(report, indent=2))
    persist()
    for index, query in enumerate(manifest['queries']):
        lock()
        name = query['file']; stem = Path(name).stem; number = query['template']
        source = (args.workload / 'queries' / name).read_bytes()
        if hashlib.sha256(source).hexdigest() != query['sha256']:
            raise ValueError('Query hash differs: ' + name)
        entry = {'mix': query['mix'], 'template': number,
                 'scored': query['mix'] in (1, 2, 3), 'status': 'fail'}
        report['queries'][name] = entry
        try:
            control = evidence.get('before_each_request')
            if control:
                if not control.get('expected_json'):
                    raise ValueError('Cache acknowledgement must be nonempty')
                raw, result = exchange(control['endpoint'], control['fields'], args.timeout)
                (args.out / (stem + '-control.body')).write_bytes(raw)
                entry['cache_control'] = result
                if result['transport_error'] or result['http_status'] != 200:
                    raise RuntimeError('Cache control failed')
                # Equality is deliberately explicit; an HTTP 200 containing an
                # error or unrelated payload cannot silently authorize timing.
                response = json.loads(raw)
                if any(response.get(k) != v for k, v in control['expected_json'].items()):
                    raise ValueError('Cache control acknowledgement differs')
            fields = dict(evidence.get('query_fields', {}))
            if 'query' in fields:
                raise ValueError('query_fields must not override the workload query')
            fields['query'] = source.decode()
            raw, result = exchange(evidence['endpoint'], fields, args.timeout)
            entry.update(result)
            (args.out / (stem + '.body')).write_bytes(raw)
            entry['body_sha256'] = hashlib.sha256(raw).hexdigest()
            if result['transport_error'] or result['http_status'] != 200:
                raise RuntimeError(result['transport_error'] or 'Unexpected HTTP status')
            actual = json.loads(raw)
            reference = json.loads((args.reference / (stem + '.json')).read_text())
            diagnostic = (json.loads((args.reference / (stem + '-diagnostic.json')).read_text())
                          if number == 7 else None)
            try:
                entry.update(verify(number, actual, reference, diagnostic, frozen, query))
                entry['status'] = 'pass'
            except Exception as exc:
                entry['error'] = type(exc).__name__ + ': ' + str(exc)
            if count_policy_sha:
                from count_equivalence import normalize
                try:
                    detail = verify(number, normalize(number, actual), normalize(number, reference),
                                    diagnostic, frozen, query)
                    entry['count_equivalence'] = {'status': 'pass', **detail}
                except Exception as exc:
                    entry['count_equivalence'] = {'status': 'fail', 'error': type(exc).__name__+': '+str(exc)}
        except Exception as exc:
            entry['error'] = type(exc).__name__ + ': ' + str(exc)
        persist()
        print(name, entry['status'], flush=True)
        if entry.get('transport_error') and entry.get('http_status') is None:
            # A client timeout/disconnect does not establish that the server's
            # query stopped. Do not enqueue later queries on an unknown state.
            report['abort_reason'] = 'Transport failed; server execution state must be checked before resuming'
            for remaining in manifest['queries'][index + 1:]:
                report['queries'][remaining['file']] = {
                    'mix': remaining['mix'], 'template': remaining['template'],
                    'scored': remaining['mix'] in (1, 2, 3), 'status': 'not_run',
                    'error': report['abort_reason']}
            persist()
            break
    report['complete'] = all(q['status'] != 'not_run' for q in report['queries'].values())
    report['ranking_eligible'] = all(q['status'] == 'pass' for q in report['queries'].values())
    # Preserve failed attempt latencies, but do not emit a total with missing or
    # incorrect answers masquerading as a complete weighted BI mix.
    report['measured_total_ns'] = (sum(q['elapsed_ns'] for q in report['queries'].values()
                                     if q['scored']) if report['ranking_eligible'] else None)
    if count_policy_sha:
        report['integer_value_equivalent_eligible'] = report['complete'] and all(
            q.get('count_equivalence', {}).get('status') == 'pass' for q in report['queries'].values())
        report['integer_value_equivalent_total_ns'] = (
            sum(q['elapsed_ns'] for q in report['queries'].values() if q['scored'])
            if report['integer_value_equivalent_eligible'] else None)
    persist()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for field in ('workload', 'reference', 'out', 'runtime-evidence', 'numeric-inputs'):
        parser.add_argument('--' + field, type=Path, required=True)
    parser.add_argument('--numeric-freeze-sha256', required=True)
    parser.add_argument('--count-policy-freeze', type=Path)
    parser.add_argument('--count-policy-sha256')
    parser.add_argument('--lock-owner', required=True)
    parser.add_argument('--timeout', type=float, default=120)
    args = parser.parse_args()
    if bool(args.count_policy_freeze) != bool(args.count_policy_sha256):
        parser.error('count policy freeze and hash must be supplied together')
    if args.timeout <= 0:
        parser.error('positive timeout required')
    run(args)
