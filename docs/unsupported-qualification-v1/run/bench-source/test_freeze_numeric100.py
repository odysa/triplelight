import copy
import unittest
from freeze_numeric100 import validate

class Freeze100Tests(unittest.TestCase):
    def setUp(self):
        self.plan={'dataset_sha256':'pinned','previously_examined_mixes':[],
          'collector_reads_candidate_outputs':False,
          'cases':{str(i):{} for i in range(28)},'instances':{str(i):{} for i in range(32)}}
        self.verified={'all_counts_agree':True,'cases':{str(i):{'counts_agree':True} for i in range(28)}}
        self.identity={'sha256':'pinned'}
    def test_complete_unseen(self):validate(self.plan,self.verified,self.identity)
    def test_wrong_graph_and_seen_outputs(self):
        with self.assertRaises(ValueError):validate(self.plan,self.verified,{'sha256':'other'})
        self.plan['previously_examined_mixes']=[0]
        with self.assertRaises(ValueError):validate(self.plan,self.verified,self.identity)
    def test_missing_or_false_case(self):
        self.verified['cases'].pop('0')
        with self.assertRaises(ValueError):validate(self.plan,self.verified,self.identity)
        self.verified['cases']['0']={'counts_agree':False}
        with self.assertRaises(ValueError):validate(self.plan,self.verified,self.identity)

if __name__=='__main__':unittest.main()
