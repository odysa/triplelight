import io
import hashlib
from pathlib import Path
import tempfile
from types import SimpleNamespace
import json
import unittest
import urllib.error
from unittest.mock import patch

from http_measure import exchange, verify, run


class Response(io.BytesIO):
    status = 200


class HttpMeasurementTests(unittest.TestCase):
    def test_complete_body_is_consumed_before_timer_stops(self):
        events = []
        class ObservedResponse(Response):
            def read(self):
                events.append('read')
                return super().read()
        def clock():
            events.append('clock')
            return len(events) * 10
        with patch('http_measure.urllib.request.urlopen', return_value=ObservedResponse(b'{"x":1}')) as call, patch('http_measure.time.perf_counter_ns', side_effect=clock):
            raw, result = exchange('http://localhost/query', {'query': 'x'}, 5)
        self.assertEqual(events, ['clock', 'read', 'clock'])
        self.assertEqual(raw, b'{"x":1}')
        self.assertEqual(result['bytes'], 7)
        self.assertIsNone(result['transport_error'])
        self.assertEqual(call.call_args.kwargs['timeout'], 5)

    def test_http_error_retains_raw_body(self):
        error = urllib.error.HTTPError('http://localhost', 500, 'query error', {}, io.BytesIO(b'engine failure'))
        with patch('http_measure.urllib.request.urlopen', side_effect=error):
            raw, result = exchange('http://localhost', {'query': 'bad'}, 5)
        self.assertEqual(raw, b'engine failure')
        self.assertEqual(result['http_status'], 500)
        self.assertIsNotNone(result['transport_error'])

    def test_timeout_is_failed_attempt(self):
        with patch('http_measure.urllib.request.urlopen', side_effect=TimeoutError('deadline')):
            raw, result = exchange('http://localhost', {}, 5)
        self.assertEqual(raw, b'')
        self.assertIsNone(result['http_status'])
        self.assertIn('TimeoutError', result['transport_error'])
        self.assertGreaterEqual(result['elapsed_ns'], 0)

    def test_q7_rejects_literal_product_before_cutoff_check(self):
        actual = {'head': {'vars': ['product']}, 'results': {'bindings': [{'product': {'type': 'literal', 'value': 'wrong'}}]}}
        with self.assertRaisesRegex(ValueError, 'shape/type'):
            verify(7, actual, {}, None, None, {})

    def test_timeout_stops_following_queries_and_suppresses_total(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); workload = root/'workload'; (workload/'queries').mkdir(parents=True)
            queries = []
            for n in range(2):
                raw = ('SELECT * WHERE {} #' + str(n)).encode(); name = str(n)+'.rq'
                (workload/'queries'/name).write_bytes(raw)
                queries.append({'file':name,'template':1,'mix':n,'sha256':hashlib.sha256(raw).hexdigest()})
            (workload/'manifest.json').write_text(json.dumps({'source_data_sha256':'data','queries':queries}))
            evidence = root/'evidence.json'
            evidence.write_text(json.dumps({'system':'test','endpoint':'http://localhost','source_or_image':'test',
                'dataset_sha256':'data','threads':1,'memory_configuration':'test','cache_profile':'none'}))
            args = SimpleNamespace(runtime_evidence=evidence, workload=workload, numeric_inputs=root/'inputs',
                numeric_freeze_sha256='freeze', out=root/'out', reference=root/'refs', lock_owner='owner',timeout=1)
            original = Path.read_text
            def read(path, *args, **kwargs):
                return 'owner' if str(path)=='/tmp/rdf-bench.lock/owner' else original(path,*args,**kwargs)
            with patch('http_measure.Path.read_text', new=read), patch('http_measure.FrozenInputs', return_value=SimpleNamespace(freeze_sha256='freeze')), patch('http_measure.exchange', return_value=(b'',{'elapsed_ns':100,'http_status':None,'bytes':0,'transport_error':'TimeoutError'})) as call:
                run(args)
            report=json.loads((args.out/'measurements.json').read_text())
            self.assertEqual(call.call_count,1)
            self.assertEqual(report['queries']['1.rq']['status'],'not_run')
            self.assertFalse(report['complete'])
            self.assertFalse(report['ranking_eligible'])
            self.assertIsNone(report['measured_total_ns'])

    def test_datatype_failure_does_not_erase_equivalent_result(self):
        from test_count_equivalence import result
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);workload=root/'workload';(workload/'queries').mkdir(parents=True)
            (workload/'queries/q.rq').write_text('query')
            import hashlib
            query={'file':'q.rq','template':1,'mix':1,'sha256':hashlib.sha256(b'query').hexdigest()}
            (workload/'manifest.json').write_text(json.dumps({'source_data_sha256':'data','queries':[query]}))
            evidence=root/'runtime.json';evidence.write_text(json.dumps({'system':'duckdb','endpoint':'http://localhost',
                'source_or_image':'test','dataset_sha256':'data','threads':1,'memory_configuration':'test','cache_profile':'none'}))
            reference=root/'refs';reference.mkdir();(reference/'q.json').write_text(json.dumps(result('12','integer')))
            policy=root/'policy.json';policy.write_text('{}')
            args=SimpleNamespace(runtime_evidence=evidence,workload=workload,numeric_inputs=root/'inputs',
                numeric_freeze_sha256='freeze',out=root/'out',reference=reference,lock_owner='owner',timeout=1,
                count_policy_freeze=policy,count_policy_sha256='policy-hash')
            original=Path.read_text
            def read(path,*args,**kwargs):
                return 'owner' if str(path)=='/tmp/rdf-bench.lock/owner' else original(path,*args,**kwargs)
            raw=json.dumps(result('12')).encode()
            with patch('http_measure.Path.read_text',new=read),patch('http_measure.FrozenInputs',return_value=SimpleNamespace(freeze_sha256='freeze')),patch('count_equivalence.load_freeze',return_value='policy-hash'),patch('http_measure.exchange',return_value=(raw,{'elapsed_ns':100,'http_status':200,'bytes':len(raw),'transport_error':None})):
                run(args)
            report=json.loads((args.out/'measurements.json').read_text())
            self.assertEqual(report['queries']['q.rq']['status'],'fail')
            self.assertEqual(report['queries']['q.rq']['count_equivalence']['status'],'pass')
            self.assertFalse(report['ranking_eligible']);self.assertIsNone(report['measured_total_ns'])
            self.assertTrue(report['integer_value_equivalent_eligible']);self.assertEqual(report['integer_value_equivalent_total_ns'],100)


if __name__ == '__main__':
    unittest.main()
