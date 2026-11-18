import unittest
from q1_partition_reference import assemble,partition_query

SOURCE='SELECT ?productType ?reviewCount {\n ?productType a bsbm:ProductType .\n} Order By desc(?reviewCount) ?productType\nLimit 10\n'
def result(types):
    return {'head':{'vars':['productType','reviewCount']},'results':{'bindings':[
      {'productType':{'type':'uri','value':v},'reviewCount':{'type':'literal','datatype':'http://www.w3.org/2001/XMLSchema#integer','value':str(n)}} for v,n in types]}}

class PartitionTests(unittest.TestCase):
    def test_values_and_domain_limit(self):
        self.assertIn('VALUES ?productType { <http://type/a> }',partition_query(SOURCE,['http://type/a']))
        for values in [[],['http://type/a']*2,[f'http://type/{i}' for i in range(9)],['http://evil/> }']]:
            with self.assertRaises(ValueError):partition_query(SOURCE,values)
    def test_top_ten_and_empty_partition(self):
        names=[f'http://type/{i:02}' for i in range(13)]
        output=assemble([(names[:8],result([(n,1) for n in names[:8]])),(names[8:],result([(n,2) for n in names[8:12]]))],names)
        self.assertEqual(len(output['results']['bindings']),10)
        self.assertEqual(output['results']['bindings'][0]['productType']['value'],names[8])
    def test_missing_domain_and_duplicate_rows(self):
        names=['http://type/a','http://type/b']
        with self.assertRaises(ValueError):assemble([(names[:1],result([(names[0],1)]))],names)
        with self.assertRaises(ValueError):assemble([(names,result([(names[0],1),(names[0],1)]))],names)

if __name__=='__main__':unittest.main()
