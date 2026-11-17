"""Semantic checks for the actual native database, not just query strings."""
import json
from pathlib import Path
import tempfile
import unittest

import native
from arrow_output import encode
from union_queries import compile_union


class NativeMapping(unittest.TestCase):
    def test_datetime_filter_rejects_plain_string(self):
        import ladybug as lb
        with tempfile.TemporaryDirectory(prefix='ladybug-date-fixture-') as folder:
            root=Path(folder);nt=root/'in.nt';source=root/'source';dbpath=root/'db'
            nt.write_text('''<urn:a> <urn:date> "2008-07-01T00:00:00"^^<http://www.w3.org/2001/XMLSchema#dateTime> .
<urn:a> <urn:date> "2008-08-01T00:00:00" .
<urn:a> <urn:date> "2008-05-01T00:00:00"^^<http://www.w3.org/2001/XMLSchema#dateTime> .
''')
            native.prepare(nt,source);native.load(source,dbpath,64,1)
            manifest=json.loads((source/'manifest.json').read_text());resources=json.loads((source/'resource-ids.json').read_text())
            rec=native.compile_bgp('SELECT ?d WHERE { <urn:a> <urn:date> ?d FILTER (?d > "2008-06-20T00:00:00"^^<http://www.w3.org/2001/XMLSchema#dateTime>) }',manifest,resources)
            db=lb.Database(str(dbpath),buffer_pool_size=64*1024**2,read_only=True,max_num_threads=1);conn=lb.Connection(db)
            result=conn.execute(rec['cypher']);got=[]
            while result.has_next():got.append(json.loads(result.get_next()[0]))
            self.assertEqual(got,[{'type':'literal','value':'2008-07-01T00:00:00','datatype':native.XSD+'dateTime'}])
            conn.close();db.close()

    def test_tuned_multiple_types_and_values(self):
        import ladybug as lb
        import tuned
        from array import array
        with tempfile.TemporaryDirectory(prefix='ladybug-tuned-fixture-') as folder:
            root=Path(folder);nt=root/'in.nt';source=root/'source';target=root/'tuned';dbpath=root/'db'
            nt.write_text('''<urn:a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Student> .
<urn:a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Person> .
<urn:a> <urn:name> "Alice" .
<urn:a> <urn:name> "Bob" .
<urn:b> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Student> .
<urn:b> <urn:name> "Carol" .
<urn:a> <urn:knows> <urn:b> .
''')
            native.prepare(nt,source);tuned.prepare(source,target);tuned.load(target,dbpath,64,1)
            manifest=json.loads((target/'manifest.json').read_text())
            self.assertEqual(manifest['counts']['unique_triples'],7)
            resources=json.loads((source/'resource-ids.json').read_text());masks=array('Q')
            with (target/'resource-masks.u64').open('rb') as f:masks.frombytes(f.read())
            db=lb.Database(str(dbpath),buffer_pool_size=64*1024**2,read_only=True,max_num_threads=1);conn=lb.Connection(db)
            for types,expected in [(['Student'],3),(['Person'],2),(['Student','Person'],2)]:
                query='SELECT ?x ?name WHERE { '+' '.join(f'?x <{native.RDF}type> <urn:{kind}> .' for kind in types)+' ?x <urn:name> ?name }'
                rec=tuned.compile_query(query,manifest,resources,masks);result=conn.execute(rec['cypher']);got=[]
                while result.has_next():got.append([json.loads(x) for x in result.get_next()])
                self.assertEqual(len(got),expected)
                self.assertTrue(all(row[1]['value'] in ('Alice','Bob','Carol') for row in got))
            rec=tuned.compile_query('SELECT ?x ?y WHERE { ?x <urn:knows> ?y }',manifest,resources,masks)
            result=conn.execute(rec['cypher']);self.assertEqual([json.loads(x)['value'] for x in result.get_next()],['urn:a','urn:b'])
            conn.close();db.close()

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
                bulk=conn.execute(rec['cypher'])
                payload=json.loads(encode(bulk.get_as_arrow(),rec))
                self.assertGreaterEqual(bulk.get_execution_time(),0)
                expected=[{v:x for v,x in zip(rec['vars'],row) if x is not None} for row in rows]
                self.assertEqual(sorted(json.dumps(x,sort_keys=True) for x in payload['results']['bindings']),sorted(json.dumps(x,sort_keys=True) for x in expected))
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
            self.assertEqual(query('SELECT ?v WHERE { <urn:absent> <urn:name> ?v }'),[])
            union='''SELECT DISTINCT ?x ?label WHERE {
            { ?x <urn:type> <urn:Student> . ?x <urn:name> ?label . ?x <urn:n> ?n . FILTER (?n > 0) }
            UNION { ?x <urn:type> <urn:Student> . ?x <urn:name> ?label . ?x <urn:label> ?tag . FILTER langMatches(lang(?tag), "EN") }
            }
            ORDER BY ?label
            LIMIT 10'''
            rec=compile_union(union,manifest,resources,native.compile_bgp,native.key)
            result=conn.execute(rec['cypher']);rows=[]
            while result.has_next():rows.append(result.get_next())
            self.assertEqual(len(rows),2)
            rec=compile_union('SELECT ?property ?hasValue ?isValueOf WHERE { { <urn:a> ?property ?hasValue } UNION { ?isValueOf ?property <urn:a> } }',manifest,resources,native.compile_bgp,native.key)
            result=conn.execute(rec['cypher']);rows=[]
            while result.has_next():rows.append(result.get_next())
            self.assertEqual(len(rows),9)  # self-loop appears once in each UNION ALL branch.
            self.assertTrue(all(r[0] in rec['predicate_labels'] for r in rows))
            payload=json.loads(encode(conn.execute(rec['cypher']).get_as_arrow(),rec))
            self.assertEqual(len(payload['results']['bindings']),9)
            selfloops=[r for r in payload['results']['bindings'] if r['property']['value']=='urn:self']
            self.assertEqual(len(selfloops),2)
            self.assertEqual(sum('hasValue' in r for r in selfloops),1)
            self.assertEqual(sum('isValueOf' in r for r in selfloops),1)
            conn.close();db.close()

    def test_unsupported_fails_closed(self):
        with self.assertRaises(ValueError):
            native.compile_bgp('SELECT ?s WHERE { { ?s <urn:p> ?o } UNION { ?s <urn:q> ?o } }',{'relationships':[],'predicates':{}},{})


if __name__=='__main__':unittest.main()
