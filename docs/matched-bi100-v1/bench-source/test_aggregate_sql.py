"""Independent tiny-graph checks; no benchmark timings or frozen BI policy edits."""
from pathlib import Path
import unittest

from aggregate_sql import translate, XSD


class AggregateSQL(unittest.TestCase):
    def setUp(self):
        import duckdb
        self.db = duckdb.connect()
        self.db.execute('CREATE TABLE triples(s VARCHAR,p VARCHAR,o STRUCT(kind UTINYINT,value VARCHAR,datatype VARCHAR,lang VARCHAR))')
        # Same numeric value but different lexical identity, plus a plain IRI.
        for s, p, o in [
            ('urn:a','urn:p',{'kind':2,'value':'1','datatype':XSD+'integer','lang':None}),
            ('urn:b','urn:p',{'kind':2,'value':'01','datatype':XSD+'integer','lang':None}),
            ('urn:c','urn:p',{'kind':0,'value':'urn:a','datatype':None,'lang':None}),
            ('urn:a','urn:q',{'kind':0,'value':'urn:a','datatype':None,'lang':None}),
            ('urn:b','urn:q',{'kind':0,'value':'urn:c','datatype':None,'lang':None}),
        ]:
            self.db.execute('INSERT INTO triples VALUES (?,?,?)',[s,p,o])

    def tearDown(self):
        self.db.close()

    def run_query(self, query):
        t = translate(query)
        return t, self.db.execute(t.sql).fetchall()

    def test_empty_global_and_empty_group(self):
        _, rows = self.run_query('SELECT (COUNT(*) AS ?n) WHERE {?s <urn:absent> ?o}')
        self.assertEqual(rows, [(0,)])
        _, rows = self.run_query('SELECT ?o (COUNT(*) AS ?n) WHERE {?s <urn:absent> ?o} GROUP BY ?o')
        self.assertEqual(rows, [])

    def test_distinct_preserves_lexical_identity(self):
        _, rows = self.run_query('SELECT (COUNT(DISTINCT ?o) AS ?n) WHERE {?s <urn:p> ?o}')
        self.assertEqual(rows, [(3,)])

    def test_cross_position_and_repeated_variable(self):
        _, rows = self.run_query('SELECT (COUNT(*) AS ?n) WHERE {?s <urn:q> ?s}')
        self.assertEqual(rows, [(1,)])
        _, rows = self.run_query('SELECT (COUNT(*) AS ?n) WHERE {?s <urn:p> ?o . ?o <urn:q> ?x}')
        self.assertEqual(rows, [(1,)])

    def test_bag_multiplicity_and_distinct(self):
        _, rows = self.run_query('SELECT (COUNT(*) AS ?n) (COUNT(DISTINCT ?s) AS ?d) WHERE {?s <urn:p> ?o . ?x <urn:p> ?y}')
        self.assertEqual(rows, [(9,3)])

    def test_group_serialization(self):
        t, rows = self.run_query('PREFIX u: <urn:> SELECT ?o (COUNT(*) AS ?n) WHERE {?s u:p ?o} GROUP BY ?o')
        document = t.result_json(rows)
        self.assertEqual(document['head']['vars'], ['o','n'])
        self.assertEqual(len(document['results']['bindings']), 3)
        literals = [b['o'] for b in document['results']['bindings'] if b['o']['type']=='literal']
        self.assertEqual({x['value'] for x in literals},{'1','01'})
        self.assertTrue(all(b['n']['datatype']==XSD+'integer' for b in document['results']['bindings']))

    def test_every_curated_query_parses(self):
        paths = sorted((Path(__file__).parent/'workloads/aggregates').rglob('*.rq'))
        self.assertEqual(len(paths),72)
        for path in paths:
            with self.subTest(path=path.name):
                self.assertTrue(translate(path.read_text()).sql.startswith('SELECT '))

    def test_unsupported_syntax_is_rejected(self):
        for query in [
            'SELECT (SUM(?o) AS ?n) WHERE {?s <urn:p> ?o}',
            'SELECT (COUNT(*) AS ?n) WHERE {?s <urn:p> ?o FILTER(?o=1)}',
            'SELECT ?s (COUNT(*) AS ?n) WHERE {?s <urn:p> ?o}',
            'SELECT (COUNT(*) AS ?n) WHERE {?s <urn:p> "x"}',
            'SELECT (COUNT(*) AS ?n) WHERE {?s <urn:p> ?o} LIMIT 1',
            'SELECT (COUNT(?missing) AS ?n) WHERE {?s <urn:p> ?o}',
        ]:
            with self.subTest(query=query), self.assertRaises(ValueError): translate(query)


if __name__ == '__main__':
    unittest.main()
