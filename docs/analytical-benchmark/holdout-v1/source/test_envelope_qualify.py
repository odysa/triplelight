import hashlib
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
from unittest.mock import patch
from fractions import Fraction as F
import unittest
from unittest.mock import patch
from types import SimpleNamespace

from envelope_qualify import qualify_enveloped,FrozenInputs
from float_envelope import InputStats

XSD='http://www.w3.org/2001/XMLSchema#'


def result(value='3.0',entity='urn:f',datatype=XSD+'float'):
    return {'head':{'vars':['feature','priceRatio']},'results':{'bindings':[{
        'feature':{'type':'uri','value':entity},
        'priceRatio':{'type':'literal','datatype':datatype,'value':value}}]}}


class ExactAnchorQualification(unittest.TestCase):
    def setUp(self):
        self.inputs={'groups':{'urn:f':{'with':InputStats.from_values([F(6),F(12)]).record(),
                                       'without':InputStats.from_values([F(2),F(4)]).record()}}}

    def test_both_engines_checked_against_exact_anchor(self):
        report=qualify_enveloped(4,result(),result(),self.inputs)
        self.assertTrue(report['reference_and_candidate_inside_envelopes'])
        for candidate,reference in [(result('30.0'),result()),(result(),result('30.0'))]:
            with self.assertRaises(ValueError):qualify_enveloped(4,candidate,reference,self.inputs)

    def test_types_and_entities_are_not_relaxed(self):
        for candidate in [result(datatype=XSD+'double'),result(entity='urn:other')]:
            with self.assertRaises(ValueError):qualify_enveloped(4,candidate,result(),self.inputs)
        with self.assertRaises(ValueError):qualify_enveloped(4,result(),result(),{'groups':{}})

    def test_unbound_and_multiplicity_errors_rejected(self):
        missing=result();del missing['results']['bindings'][0]['priceRatio']
        with self.assertRaises(ValueError):qualify_enveloped(4,missing,result(),self.inputs)
        duplicate=result();duplicate['results']['bindings']*=2
        with self.assertRaises(ValueError):qualify_enveloped(4,duplicate,result(),self.inputs)

    def test_empty_rows_and_duplicate_projection(self):
        empty=result();empty['results']['bindings']=[]
        self.assertEqual(qualify_enveloped(4,empty,empty,{'groups':{}})['rows'],0)
        invalid=result();invalid['head']['vars'].append('feature')
        with self.assertRaises(ValueError):qualify_enveloped(4,invalid,result(),self.inputs)

    def test_product_and_reviewer_mean_keys_and_exact_counts(self):
        product={'head':{'vars':['country','product','nrOfReviews','avgPrice']},'results':{'bindings':[{
            'country':{'type':'uri','value':'urn:country'},'product':{'type':'uri','value':'urn:p'},
            'nrOfReviews':{'type':'literal','datatype':XSD+'integer','value':'7'},
            'avgPrice':{'type':'literal','datatype':XSD+'float','value':'3.0'}}]}}
        inputs={'groups':{'urn:p':InputStats.from_values([F(2),F(4)]).record()}}
        self.assertTrue(qualify_enveloped(5,product,product,inputs)['reference_and_candidate_inside_envelopes'])
        wrong=json.loads(json.dumps(product));wrong['results']['bindings'][0]['nrOfReviews']['value']='8'
        with self.assertRaises(ValueError):qualify_enveloped(5,wrong,product,inputs)
        reviewer={'head':{'vars':['reviewer','reviewerAvgScore']},'results':{'bindings':[{
            'reviewer':{'type':'uri','value':'urn:r'},
            'reviewerAvgScore':{'type':'literal','datatype':XSD+'float','value':'3.0'}}]}}
        self.assertTrue(qualify_enveloped(6,reviewer,reviewer,{'groups':{'urn:r':inputs['groups']['urn:p']}})['reference_and_candidate_inside_envelopes'])

    def test_frozen_input_changes_are_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);(root/'sql').mkdir()
            plan={'dataset_sha256':'data','workload_manifest_sha256':'workload',
                  'instances':{'q.rq':{'case':'case','query_sha256':'query'}}}
            (root/'plan.json').write_text(json.dumps(plan))
            digest=lambda path:hashlib.sha256(path.read_bytes()).hexdigest()
            (root/'count-verification.json').write_text(json.dumps({'all_counts_agree':True,'plan_sha256':digest(root/'plan.json')}))
            (root/'sql/case.json').write_text(json.dumps(self.inputs))
            names=['envelope_qualify.py','float_envelope.py','qualify.py','workload_check.py','numeric_inputs.py','bi_sql.py']
            frozen={'state':'frozen','validator_sources':{name:digest(Path(__file__).parent/name) for name in names},
                    'input_files':{name:digest(root/name) for name in ['plan.json','count-verification.json','sql/case.json']}}
            (root/'freeze.json').write_text(json.dumps(frozen));identity=digest(root/'freeze.json')
            loaded=FrozenInputs(root,identity,'data','workload')
            self.assertEqual(loaded.for_instance({'file':'q.rq','sha256':'query'}),self.inputs)
            with self.assertRaises(ValueError):loaded.for_instance({'file':'q.rq','sha256':'changed'})
            (root/'sql/case.json').write_text('{}')
            with self.assertRaises(ValueError):FrozenInputs(root,identity,'data','workload')

    def test_failed_reference_output_is_preserved(self):
        import workload_check
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);workload=root/'workload';(workload/'queries').mkdir(parents=True)
            source='SELECT ?feature ?priceRatio WHERE {}'
            (workload/'queries/q4.rq').write_text(source)
            manifest={'source_data_sha256':'data','queries':[{'file':'q4.rq','template':4,
                       'sha256':hashlib.sha256(source.encode()).hexdigest(),'parameters':{}}]}
            (workload/'manifest.json').write_text(json.dumps(manifest))
            args=SimpleNamespace(workload=workload,out=root/'result',system='jena',dataset_sha256='data',
                                 runtime_source='unit fixture',limit=0,lock_owner='unit-owner',
                                 numeric_inputs=root/'inputs',numeric_freeze_sha256='unit-hash',
                                 endpoint='unit',timeout=1)
            args=SimpleNamespace(workload=workload,out=root/'results',dataset_sha256='data',
                 limit=0,system='jena',runtime_source='mock-reference',lock_owner='unit-owner',
                 numeric_inputs=root/'inputs',numeric_freeze_sha256='unit-freeze',endpoint='mock',timeout=1)
            actual=result('30.0')
            def paths(value):
                if str(value)=='/tmp/rdf-bench.lock/owner':return SimpleNamespace(read_text=lambda:'unit-owner')
                return Path(value)
            with patch.object(workload_check,'Path',side_effect=paths),patch.object(workload_check,'ask',return_value=actual),patch('envelope_qualify.FrozenInputs') as loader:
                loader.return_value.freeze_sha256='unit-hash'
                loader.return_value.for_instance.return_value=self.inputs
                workload_check.run(args)
            matrix=json.loads((args.out/'matrix.json').read_text())
            self.assertFalse(matrix['all_requested_pass'])
            self.assertEqual(matrix['queries']['q4.rq']['status'],'fail')
            self.assertEqual(json.loads((args.out/'q4.json').read_text()),actual)


if __name__=='__main__':unittest.main()
