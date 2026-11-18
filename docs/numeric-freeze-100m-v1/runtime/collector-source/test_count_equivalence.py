import unittest
from count_equivalence import normalize, XSD
from qualify import qualify


def result(value, datatype='int'):
    return {'head':{'vars':['productType','reviewCount']},'results':{'bindings':[
        {'productType':{'type':'uri','value':'urn:product-type'},
         'reviewCount':{'type':'literal','datatype':XSD+datatype,'value':value}}]}}


class CountEquivalenceTests(unittest.TestCase):
    def test_strict_failure_and_equivalent_pass_are_distinct(self):
        actual=result('12');reference=result('12','integer')
        with self.assertRaisesRegex(ValueError,'datatype'):qualify(1,actual,reference)
        self.assertTrue(qualify(1,normalize(1,actual),normalize(1,reference))['all_columns_compatible'])
        self.assertEqual(actual['results']['bindings'][0]['reviewCount']['datatype'],XSD+'int')

    def test_value_change_is_not_excused(self):
        with self.assertRaises(ValueError):qualify(1,normalize(1,result('13')),normalize(1,result('12','integer')))

    def test_non_count_literals_are_unchanged(self):
        self.assertEqual(normalize(6,result('12')),result('12'))

    def test_lexical_equivalence_is_limited_to_valid_integer_forms(self):
        self.assertEqual(normalize(1,result('+0012'))['results']['bindings'][0]['reviewCount']['value'],'12')
        for invalid in [' 12','1_2','1.0','-1','2147483648']:
            with self.subTest(invalid=invalid),self.assertRaises(ValueError):normalize(1,result(invalid))

    def test_other_integer_datatypes_are_not_admitted(self):
        for datatype in ['long','decimal','double']:
            with self.subTest(datatype=datatype),self.assertRaises(ValueError):normalize(1,result('12',datatype))


if __name__=='__main__':unittest.main()
