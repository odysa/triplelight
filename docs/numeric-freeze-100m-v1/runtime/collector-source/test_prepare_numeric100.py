import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from prepare_numeric100 import prepare


class Numeric100Preparation(unittest.TestCase):
    def test_new_graph_scope_and_all_bindings_are_retained(self):
        workload = Path(__file__).parent/'workloads/bsbm100'
        manifest = json.loads((workload/'manifest.json').read_text())
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)/'plan'
            plan = prepare(workload, out)
            self.assertEqual(plan['previously_examined_mixes'], [])
            self.assertEqual(plan['unseen_confirmatory_mixes'], [1,2,3])
            self.assertFalse(plan['validator_frozen'])
            self.assertFalse(plan['collector_reads_candidate_outputs'])
            expected = {q['file']: q['sha256'] for q in manifest['queries'] if q['template'] in (4,5,6)}
            self.assertEqual({name: value['query_sha256'] for name,value in plan['instances'].items()}, expected)
            for case in plan['cases'].values():
                for kind in ('sql','jena'):
                    for record in case[kind].values():
                        self.assertEqual(hashlib.sha256((out/record['file']).read_bytes()).hexdigest(), record['sha256'])
            with self.assertRaises(FileExistsError): prepare(workload, out)

    def test_old_10m_workload_cannot_be_relabelled_100m(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(ValueError):
                prepare(Path(__file__).parent/'workloads/bsbm10', Path(tmp)/'wrong')
            self.assertFalse((Path(tmp)/'wrong').exists())


if __name__ == '__main__': unittest.main()
