"""Semantic checks for the actual native database, not just query strings."""
import json
from pathlib import Path
import tempfile
import unittest

import native
from union_queries import compile_union


class NativeMapping(unittest.TestCase):
    def test_terms_bags_optional_and_numeric(self):
        import ladybug as lb
        with tempfile.TemporaryDirectory(prefix='ladybug-fixture-') as folder:
            root=Path(folder);nt=root/'in.nt';csvdir=root/'csv';dbpath=root/'db'
            nt.write_text('''<urn:a> <urn:type> <urn:Student> .
<urn:a> <urn:type> <urn:Person> .
<urn:a> <urn:name> "A" .
<urn:a> <urn:name> "A"^^<http://www.w3.org/2001/XMLSchema#string> .
<urn:a> <urn:name> "B" .
<urn:a> <urn:n> "1"^^<http://www.w3.org/2001/XMLSchema#int> .
<urn:a> <urn:n> "01"^^<http://www.w3.org/2001/XMLSchema#int> .
<urn:b> <urn:type> <urn:Student> .
<urn:a> <urn:label> "hi"@EN .
<urn:a> <urn:self> <urn:a> .
''')
            native.prepare(nt,csvdir);native.load(csvdir,dbpath,64,1)
            manifest=json.loads((csvdir/'manifest.json').read_text())
            self.assertEqual(manifest['counts']['source_lines'],10)
            self.assertEqual(manifest['counts']['unique_triples'],9)
            resources=json.loads((csvdir/'resource-ids.json').read_text())
            db=lb.Database(str(dbpath),buffer_pool_size=64*1024**2,read_only=True,max_num_threads=1)
            conn=lb.Connection(db)
            def query(q):
                rec=native.compile_bgp(q,manifest,resources)
                result=conn.execute(rec['cypher']);rows=[]
                while result.has_next():rows.append([json.loads(x) if x else None for x in result.get_next()[:len(rec['vars'])]])
                return rows
            # Projection retains the two bindings caused by a multi-valued name.
            rows=query('SELECT ?x WHERE { ?x <urn:type> <urn:Student> . ?x <urn:name> ?n }')
            self.assertEqual([r[0]['value'] for r in rows],['urn:a','urn:a'])
            rows=query('SELECT ?x ?n WHERE { ?x <urn:type> <urn:Student> OPTIONAL { ?x <urn:name> ?n } }')
            self.assertEqual(len(rows),3)
            self.assertEqual(sum(r[1] is None for r in rows),1)
            rows=query('SELECT DISTINCT ?n WHERE { <urn:a> <urn:n> ?n FILTER (?n > 0) }')
            self.assertEqual({r[0]['value'] for r in rows},{'1','01'})
            rows=query('SELECT ?x WHERE { ?x <urn:type> <urn:Student> OPTIONAL { ?x <urn:name> ?n } FILTER (!bound(?n)) }')
            self.assertEqual([r[0]['value'] for r in rows],['urn:b'])
            rows=query('SELECT ?v WHERE { <urn:a> <urn:label> ?v FILTER langMatches(lang(?v), "EN") }')
            self.assertEqual(rows[0][0]['xml:lang'],'en')
            union='''SELECT DISTINCT ?x ?label WHERE {
            { ?x <urn:type> <urn:Student> . ?x <urn:name> ?label . ?x <urn:n> ?n . FILTER (?n > 0) }
            UNION { ?x <urn:type> <urn:Student> . ?x <urn:name> ?label . ?x <urn:label> ?tag . FILTER langMatches(lang(?tag), "EN") }
            } ORDER BY ?label LIMIT 10'''
            rec=compile_union(union,manifest,resources,native.compile_bgp,native.key)
            result=conn.execute(rec['cypher']);rows=[]
            while result.has_next():rows.append(result.get_next())
            self.assertEqual(len(rows),2)
            rec=compile_union('SELECT ?property ?hasValue ?isValueOf WHERE { { <urn:a> ?property ?hasValue } UNION { ?isValueOf ?property <urn:a> } }',manifest,resources,native.compile_bgp,native.key)
            result=conn.execute(rec['cypher']);rows=[]
            while result.has_next():rows.append(result.get_next())
            self.assertEqual(len(rows),9)  # self-loop appears once in each UNION ALL branch.
            self.assertTrue(all(r[0] in rec['predicate_labels'] for r in rows))
            conn.close();db.close()

    def test_unsupported_fails_closed(self):
        with self.assertRaises(ValueError):
            native.compile_bgp('SELECT ?s WHERE { { ?s <urn:p> ?o } UNION { ?s <urn:q> ?o } }',{'relationships':[],'predicates':{}},{})


if __name__=='__main__':unittest.main()
