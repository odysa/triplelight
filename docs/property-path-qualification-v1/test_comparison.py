import copy, unittest
from qualify_paths import compare
U=lambda v:{'type':'uri','value':v}
def result(rows):return {'head':{'vars':['s','o']},'results':{'bindings':rows}}
class ComparisonControls(unittest.TestCase):
 def test_duplicate_is_not_a_set(self):
  r={'s':U('urn:a'),'o':U('urn:b')};self.assertFalse(compare(result([r]),result([r,r])))
 def test_bag_ignores_row_order(self):
  a={'s':U('urn:a')};b={'s':U('urn:b')};self.assertTrue(compare(result([a,b]),result([b,a])))
 def test_ordered_case_detects_reversal(self):
  a={'s':U('urn:a')};b={'s':U('urn:b')};self.assertFalse(compare(result([a,b]),result([b,a]),True))
 def test_unbound_not_an_rdf_value(self):
  self.assertFalse(compare(result([{}]),result([{'s':U('urn:unbound')}])) )
 def test_numeric_lexical_identity(self):
  def row(v):return {'o':{'type':'literal','value':v,'datatype':'http://www.w3.org/2001/XMLSchema#int'}}
  self.assertFalse(compare(result([row('01')]),result([row('1')])))
 def test_plain_string_is_xsd_string(self):
  a={'o':{'type':'literal','value':'x'}};b={'o':{'type':'literal','value':'x','datatype':'http://www.w3.org/2001/XMLSchema#string'}};self.assertTrue(compare(result([a]),result([b])))
 def test_ask_false_not_empty_select(self):self.assertFalse(compare({'boolean':False},result([])))
 def test_projection_is_checked(self):
  a=result([]);b=copy.deepcopy(a);b['head']['vars']=['s'];self.assertFalse(compare(a,b))
if __name__=='__main__':unittest.main()
