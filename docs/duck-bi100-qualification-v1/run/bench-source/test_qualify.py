"""Offline semantic-oracle checks, not database performance simulations."""
import struct
import unittest
from qualify import XSD, qualify, q7_allowed_products, check_order
from make_workload import recover_parameters


def iri(s):return {'type':'uri','value':s}
def floating(s):return {'type':'literal','value':s,'datatype':XSD+'float'}
def result(rows):return {'head':{'vars':['product','ratio']},'results':{'bindings':rows}}


class Qualification(unittest.TestCase):
    def test_computed_values_types_and_exact_representation(self):
        a=result([{'product':iri('urn:a'),'ratio':floating('1.0')}])
        b=result([{'product':iri('urn:a'),'ratio':floating('1E0')}])
        check=qualify(3,a,b)
        self.assertTrue(check['all_columns_compatible']);self.assertFalse(check['exact_term_bag'])
        wrong=result([{'product':iri('urn:a'),'ratio':floating('1.1')}])
        with self.assertRaises(ValueError):qualify(3,a,wrong)
        b['results']['bindings'][0]['ratio']['datatype']=XSD+'double'
        with self.assertRaises(ValueError):qualify(3,a,b)

    def test_declared_float_ulp_bound(self):
        bits=struct.unpack('>I',struct.pack('>f',1.0))[0]
        def value(offset):return str(struct.unpack('>f',struct.pack('>I',bits+offset))[0])
        a=result([{'product':iri('urn:a'),'ratio':floating('1')}])
        b=result([{'product':iri('urn:a'),'ratio':floating(value(4))}])
        self.assertEqual(qualify(3,a,b)['computed_float_max_ulps'],4)
        b['results']['bindings'][0]['ratio']=floating(value(5))
        with self.assertRaises(ValueError):qualify(3,a,b)

    def test_cutoff_ties_before_filter(self):
        rows=[{'product':'must','offerCount':10,'blocked':False},
              {'product':'filtered','offerCount':10,'blocked':True},
              {'product':'a','offerCount':5,'blocked':False},
              {'product':'b','offerCount':5,'blocked':False},
              {'product':'c','offerCount':5,'blocked':True}]
        # Three inner slots: two mandatory high counts + one arbitrary boundary product.
        q7_allowed_products(['must'],rows,3)  # selecting c is then filtered out
        q7_allowed_products(['must','a'],rows,3)
        q7_allowed_products(['must','b'],rows,3)
        for bad in ([],['a'],['must','a','b'],['must','filtered'],['must','must']):
            with self.assertRaises(ValueError):q7_allowed_products(bad,rows,3)

    def test_numeric_outer_order_and_null_last_descending(self):
        ordered=result([{'product':iri('urn:a'),'ratio':floating('10')},
                        {'product':iri('urn:b'),'ratio':floating('2')}, {'product':iri('urn:c')}])
        self.assertTrue(check_order(3,ordered)['checked'])
        ordered['results']['bindings'].reverse()
        with self.assertRaises(ValueError):check_order(3,ordered)

    def test_repeated_parameter_fill_identity(self):
        source='A %Product% B %Product% C "%Date%"'
        query='A <urn:a> B <urn:a> C "2008-01-01"'
        self.assertEqual(recover_parameters(source,query),{'Product':'<urn:a>','Date':'2008-01-01'})
        with self.assertRaises(ValueError):recover_parameters(source,query.replace('B <urn:a>','B <urn:b>'))


if __name__=='__main__':unittest.main()
