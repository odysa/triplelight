import argparse
import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import http_measure_value as gate
from qualify import XSD

class TimingGateTests(unittest.TestCase):
    def exercise(self,case):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);work=root/'work';ref=root/'ref';out=root/'out'
            for p in (work,ref,out):p.mkdir()
            query={'file':'q.rq','mix':1,'template':3};manifest={'source_data_sha256':'fixture','queries':[query]}
            if case=='failed-warmup':manifest['queries'].append({'file':'warm.rq','mix':0,'template':3})
            (work/'manifest.json').write_text(json.dumps(manifest))
            reference={'head':{'vars':['product','ratio']},'results':{'bindings':[{'product':{'type':'uri','value':'urn:p'},'ratio':{'type':'literal','datatype':XSD+'float','value':'1'}}]}}
            actual=copy.deepcopy(reference);actual['results']['bindings'][0]['ratio']['datatype']=XSD+'decimal'
            raw=json.dumps(actual).encode();rraw=json.dumps(reference).encode()
            hashes={};queries={}
            for q in manifest['queries']:
                stem=Path(q['file']).stem;body=raw
                if case=='wrong-bag' or (case=='failed-warmup' and q['mix']==0):
                    bad=copy.deepcopy(actual);bad['results']['bindings'][0]['product']['value']='urn:bad';body=json.dumps(bad).encode()
                (out/(stem+'.body')).write_bytes(body);(ref/(stem+'.json')).write_bytes(rraw);hashes[stem+'.json']=gate.sha(rraw)
                queries[q['file']]={'status':'fail','mix':q['mix'],'template':3,'scored':q['mix']==1,'http_status':200,'transport_error':None,'body_sha256':gate.sha(body),'elapsed_ns':123,'error':'Original datatype rejection'}
            if case=='transport':queries['q.rq'].update(transport_error='timeout',http_status=None)
            if case=='not-run':queries['q.rq']['status']='not_run'
            original={'complete':case!='not-run','queries':queries,'ranking_eligible':False,'measured_total_ns':None}
            (out/'measurements.json').write_text(json.dumps(original))
            policy_path=root/'policy.json';policy_path.write_text('{}')
            args=argparse.Namespace(workload=work,reference=ref,out=out,numeric_inputs=root,numeric_freeze_sha256='n',count_policy_sha256='c',value_policy_freeze=policy_path,value_policy_sha256='p')
            policy={'numeric_freeze_sha256':'n','count_freeze_sha256':'c','reference_files':hashes}
            with patch.object(gate,'check_policy',return_value=policy),patch.object(gate,'FrozenInputs'),patch.object(gate.http_measure,'run'):
                gate.run(args)
            result=json.loads((out/'measurements.json').read_text())
            self.assertFalse(result['ranking_eligible']);self.assertIsNone(result['measured_total_ns'])
            for k,v in queries.items():
                self.assertEqual(v,{key:value for key,value in result['queries'][k].items() if key!='value_compatibility'})
            return result
    def test_value_pass_preserves_strict_failure(self):
        r=self.exercise('pass');self.assertTrue(r['value_compatible_eligible']);self.assertEqual(r['value_compatible_total_ns'],123)
    def test_wrong_bag_blocks_total(self):self.reject('wrong-bag')
    def test_timeout_blocks_total(self):self.reject('transport')
    def test_not_run_blocks_total(self):self.reject('not-run')
    def test_bad_warmup_blocks_measured_total(self):self.reject('failed-warmup')
    def reject(self,case):
        r=self.exercise(case);self.assertFalse(r['value_compatible_eligible']);self.assertIsNone(r['value_compatible_total_ns'])
    def test_policy_hash_fails_closed(self):
        with tempfile.TemporaryDirectory() as t:
            p=Path(t)/'freeze.json';p.write_text('{}')
            with self.assertRaisesRegex(ValueError,'hash'):gate.check_policy(p,'wrong','workload')
if __name__=='__main__':unittest.main()
