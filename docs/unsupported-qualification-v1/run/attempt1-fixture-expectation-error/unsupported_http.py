import json,urllib.request,urllib.parse,urllib.error
from pathlib import Path
stage=Path(__file__).resolve().parent
cases=[]
def reject(name,query):cases.append((name,query,501))
for fn in ['NOW()','RAND()','UUID()','STRUUID()','BNODE()','BNODE("label")','MD5("a")','SHA1("a")','SHA256("a")','SHA384("a")','SHA512("a")',
           'SECONDS("2000-01-01T01:02:03Z"^^<http://www.w3.org/2001/XMLSchema#dateTime>)',
           'TIMEZONE("2000-01-01T01:02:03Z"^^<http://www.w3.org/2001/XMLSchema#dateTime>)',
           'TZ("2000-01-01T01:02:03Z"^^<http://www.w3.org/2001/XMLSchema#dateTime>)','<urn:custom>(1)']:
    reject('function-'+str(len(cases)),f'SELECT ({fn} AS ?v) WHERE {{}}')
for name,q in [
 ('graph-select','SELECT * WHERE { GRAPH <urn:g> { ?s ?p ?o } }'),
 ('graph-ask','ASK { GRAPH ?g { ?s ?p ?o } }'),
 ('graph-construct','CONSTRUCT { ?s ?p ?o } WHERE { GRAPH ?g { ?s ?p ?o } }'),
 ('graph-describe','DESCRIBE ?s WHERE { GRAPH ?g { ?s ?p ?o } }'),
 ('dataset-from','SELECT * FROM <urn:g> WHERE { ?s ?p ?o }'),
 ('dataset-named','SELECT * FROM NAMED <urn:g> WHERE { ?s ?p ?o }'),
 ('dataset-ask','ASK FROM <urn:g> { ?s ?p ?o }'),
 ('path-plus','SELECT * WHERE { ?s <urn:p>+ ?o }'),
 ('path-sequence','SELECT * WHERE { ?s <urn:p>/<urn:q> ?o }'),
 ('service','SELECT * WHERE { SERVICE <http://127.0.0.1:1/sparql> { ?s ?p ?o } }'),
 ('service-silent','SELECT * WHERE { SERVICE SILENT <http://127.0.0.1:1/sparql> { ?s ?p ?o } }'),
 ('empty-bind','SELECT ?v WHERE { VALUES ?x {} BIND(NOW() AS ?v) }'),
 ('false-filter','SELECT * WHERE { FILTER(false && (RAND() > 0)) }'),
 ('if-dead-branch','SELECT (IF(true, 1, NOW()) AS ?v) WHERE {}'),
 ('coalesce-dead-branch','SELECT (COALESCE(1, NOW()) AS ?v) WHERE {}'),
 ('optional-empty-left','SELECT * WHERE { VALUES ?x {} OPTIONAL { BIND(NOW() AS ?v) } }'),
 ('minus-empty-left','SELECT * WHERE { VALUES ?x {} MINUS { BIND(NOW() AS ?v) } }'),
 ('union','SELECT ?v WHERE { { BIND(1 AS ?v) } UNION { BIND(NOW() AS ?v) } }'),
 ('exists','SELECT * WHERE { FILTER EXISTS { BIND(NOW() AS ?v) } }'),
 ('not-exists','SELECT * WHERE { FILTER NOT EXISTS { BIND(NOW() AS ?v) } }'),
 ('aggregate-argument','SELECT (SUM(RAND()) AS ?v) WHERE { VALUES ?x {1 2} }'),
 ('order-expression','SELECT ?x WHERE { VALUES ?x {1 2} } ORDER BY RAND()'),
 ('having','SELECT (COUNT(*) AS ?n) WHERE {} HAVING(RAND()>0)'),
 ('nested-select','SELECT * WHERE { { SELECT (NOW() AS ?v) WHERE {} } }'),
]:reject(name,q)
cases.extend([
 ('accepted-select','SELECT (STRLEN("abc") AS ?v) WHERE {}',200),
 ('accepted-xsd-cast','SELECT (<http://www.w3.org/2001/XMLSchema#integer>("12") AS ?v) WHERE {}',200),
 ('accepted-error-coalesce','SELECT (COALESCE(1/0,2) AS ?v) WHERE {}',200),
 ('accepted-ask','ASK {}',200),
 ('accepted-empty-values','SELECT ?x WHERE { VALUES ?x {} }',200),
])
out={'cases':{},'complete':False}
for name,query,expected in cases:
    request=urllib.request.Request('http://127.0.0.1:31436/sparql',data=urllib.parse.urlencode({'query':query}).encode(),headers={'Accept':'application/sparql-results+json','Content-Type':'application/x-www-form-urlencoded'})
    try:
        with urllib.request.urlopen(request,timeout=10) as response:code=response.status;body=response.read()
    except urllib.error.HTTPError as response:code=response.code;body=response.read()
    (stage/'fixtures').mkdir(exist_ok=True)
    (stage/'fixtures'/(name+'.rq')).write_text(query)
    (stage/'fixtures'/(name+'.body')).write_bytes(body)
    out['cases'][name]={'expected_http':expected,'actual_http':code,'pass':code==expected}
    if code==200:
        result=json.loads(body)
        if name=='accepted-select':assert result['results']['bindings'][0]['v']['value']=='3'
        if name=='accepted-xsd-cast':assert result['results']['bindings'][0]['v']['value']=='12'
        if name=='accepted-error-coalesce':assert result['results']['bindings'][0]['v']['value']=='2'
        if name=='accepted-ask':assert result['boolean'] is True
        if name=='accepted-empty-values':assert result['results']['bindings']==[]
    (stage/'fixture-results.json').write_text(json.dumps(out,indent=2))
out['complete']=True;out['all_pass']=all(v['pass'] for v in out['cases'].values())
(stage/'fixture-results.json').write_text(json.dumps(out,indent=2))
assert out['all_pass'],[(n,v) for n,v in out['cases'].items() if not v['pass']]
