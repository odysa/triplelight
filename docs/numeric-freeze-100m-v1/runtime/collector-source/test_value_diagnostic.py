import copy
from fractions import Fraction
import unittest
from value_diagnostic import numeric_value, diagnose
from qualify import XSD

def term(v,kind='decimal'): return {'type':'literal','value':v,'datatype':XSD+kind}
def result(v): return {'head':{'vars':['product','ratio']},'results':{'bindings':[{'product':{'type':'uri','value':'urn:p'},'ratio':v}]}}
class ValueDiagnosticTests(unittest.TestCase):
    def test_value_spaces(self):
        self.assertEqual(numeric_value(term('0.1')),Fraction(1,10))
        self.assertEqual(numeric_value(term('0.1','double')),Fraction.from_float(0.1))
        self.assertNotEqual(numeric_value(term('0.1','float')),numeric_value(term('0.1','double')))
    def test_reject_invalid(self):
        for v in [term('1e2'),term('NaN','double'),term('INF','float'),term('128','byte'),term('-1','positiveInteger'),term('1','string')]:
            with self.assertRaises(ValueError): numeric_value(v)
    def test_four_steps_not_narrowed(self):
        ref=result(term('1','float'))
        diagnose(3,result(term('1.000000476837158203125')),ref)
        # Only 2**-60 beyond upper bound: narrowing to float would hide this.
        with self.assertRaisesRegex(ValueError,'four-step'):
            diagnose(3,result(term(str(float(1+4*2**-23)+2**-50),'double')),ref)
    def test_wrong_entity_and_multiplicity(self):
        ref=result(term('1','float'));actual=copy.deepcopy(ref)
        actual['results']['bindings'][0]['product']['value']='urn:wrong'
        with self.assertRaisesRegex(ValueError,'bags'): diagnose(3,actual,ref)
        actual=copy.deepcopy(ref);actual['results']['bindings']*=2
        with self.assertRaisesRegex(ValueError,'bags'): diagnose(3,actual,ref)
    def test_bound_unbound(self):
        with self.assertRaisesRegex(ValueError,'bound/unbound'): diagnose(3,result(None),result(term('1','float')))
    def test_duplicate_count_rows(self):
        ref={'head':{'vars':['reviewCount']},'results':{'bindings':[{'reviewCount':term('2','integer')}]}}
        actual=copy.deepcopy(ref);actual['results']['bindings']*=2
        with self.assertRaisesRegex(ValueError,'bags'): diagnose(1,actual,ref)
    def test_counts_any_exact_numeric_value(self):
        ref={'head':{'vars':['reviewCount']},'results':{'bindings':[{'reviewCount':term('2','integer')}]}}
        actual=copy.deepcopy(ref);actual['results']['bindings'][0]['reviewCount']=term('2.0','double')
        diagnose(1,actual,ref)
        actual['results']['bindings'][0]['reviewCount']=term('2.01')
        with self.assertRaisesRegex(ValueError,'COUNT'): diagnose(1,actual,ref)
    def test_envelope_no_narrowing(self):
        ref={'head':{'vars':['reviewer','reviewerAvgScore']},'results':{'bindings':[{'reviewer':{'type':'uri','value':'urn:r'},'reviewerAvgScore':term('1','float')}]}}
        inputs={'groups':{'urn:r':{'n':1,'exact_sum':'1','exact_sum_magnitudes':'1'}}}
        diagnose(6,ref,ref,inputs)
        actual=copy.deepcopy(ref);actual['results']['bindings'][0]['reviewerAvgScore']=term('1.00001')
        with self.assertRaisesRegex(ValueError,'envelope'): diagnose(6,actual,ref,inputs)
    def test_order_report_separate(self):
        ref=result(term('1','float')); row=copy.deepcopy(ref['results']['bindings'][0]);row['ratio']=term('2','float');row['product']['value']='urn:q'
        ref['results']['bindings'].append(row)
        out=diagnose(3,ref,ref);self.assertEqual(out['value_bag'],'pass');self.assertEqual(out['order']['status'],'fail')
if __name__=='__main__': unittest.main()
