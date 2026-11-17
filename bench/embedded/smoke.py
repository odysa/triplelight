#!/usr/bin/env python3
"""Real in-process fixture on both engines; run only with the benchmark lock."""
import argparse
import json
from pathlib import Path
import sys
import tempfile

from contract import XSD, json_term, python_term, signature, term


def run(owner):
    if Path('/tmp/rdf-bench.lock/owner').read_text().strip() != owner:
        raise RuntimeError('Own the shared benchmark lock first')
    import ladybug
    import rdfdb
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'ladybug'))
    import native
    with tempfile.TemporaryDirectory(prefix='rdf-jianlin-embedded-smoke-') as folder:
        root = Path(folder)
        nt = root / 'fixture.nt'
        nt.write_text('''<urn:a> <urn:n> "1"^^<http://www.w3.org/2001/XMLSchema#int> .
<urn:a> <urn:n> "01"^^<http://www.w3.org/2001/XMLSchema#int> .
<urn:a> <urn:n> "1"^^<http://www.w3.org/2001/XMLSchema#integer> .
<urn:a> <urn:empty> "" .
<urn:a> <urn:label> "hi"@EN .
<urn:a> <urn:type> <urn:T1> .
<urn:a> <urn:type> <urn:T2> .
<urn:b> <urn:missing> "present only on b" .
<urn:a> <urn:price> "10"^^<urn:USD> .
<urn:a> <urn:price> "2"^^<urn:USD> .
<urn:a> <urn:price> "1"^^<urn:USD> .
''')
        native.prepare(nt, root/'csv')
        native.load(root/'csv', root/'native', 64, 1)
        manifest = json.loads((root/'csv/manifest.json').read_text())
        resources = json.loads((root/'csv/resource-ids.json').read_text())
        db = rdfdb.Database(str(root/'rdf'), initial=str(nt))
        lb = ladybug.Database(str(root/'native'), read_only=True, buffer_pool_size=64*1024**2, max_num_threads=1)
        conn = ladybug.Connection(lb)
        cases = [
            ('SELECT ?n WHERE { <urn:a> <urn:n> ?n }', ['n'],
             [(term('literal','1',XSD+'int'),), (term('literal','01',XSD+'int'),),
              (term('literal','1',XSD+'integer'),)]),
            ('SELECT ?e ?missing WHERE { <urn:a> <urn:empty> ?e OPTIONAL { <urn:a> <urn:missing> ?missing } }',
             ['e','missing'], [(term('literal',''),None)]),
            ('SELECT ?label WHERE { <urn:a> <urn:label> ?label . <urn:a> <urn:type> ?t }',
             ['label'], [(term('literal','hi',lang='en'),)]*2),
            ('PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> SELECT ?price WHERE { <urn:a> <urn:price> ?price } ORDER BY xsd:double(str(?price))',
             ['price'], [(term('literal',n,'urn:USD'),) for n in ('1','2','10')])]
        try:
            for query, variables, expected in cases:
                wanted = signature(variables, expected)
                result = db.query(query)
                object_rows = [tuple(python_term(t,rdfdb) for t in row) for row in result]
                assert signature(result.vars, object_rows) == wanted
                raw = json.loads(db.query_json(query))
                json_rows = [tuple(json_term(row.get(v)) for v in variables) for row in raw['results']['bindings']]
                assert signature(raw['head']['vars'], json_rows) == wanted
                rec = native.compile_bgp(query, manifest, resources)
                result = conn.execute(rec['cypher']).get_as_arrow()
                rows = [tuple(json_term(json.loads(v)) if v is not None else None for v in row)
                        for row in zip(*(result.column(v).to_pylist() for v in rec['vars']))]
                assert signature(rec['vars'], rows) == wanted
                if 'ORDER BY' in query:
                    assert object_rows == expected and json_rows == expected and rows == expected
            print('PASS: 4 actual cross-engine fixtures, rdfdb objects + JSON + Ladybug Arrow, including USD-cast order')
        finally:
            conn.close(); lb.close(); db.close()


if __name__ == '__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--lock-owner',required=True)
    run(p.parse_args().lock_owner)
