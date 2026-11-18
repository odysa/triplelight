import collections,hashlib,json,os,signal,subprocess,sys
from pathlib import Path
stage=Path(__file__).resolve().parent;root=stage.parent
sys.path.insert(0,str(root/'w3c-string-ef7a778/bench-source'))
from qualify import exact_term
jena=Path('<agent-workspace>/rdfdb/bench/tools/apache-jena-6.2.0')
cases={}
for name in ['undef-bind','bound-bind','undef-filter','undef-optional','union-maybe-bound','subquery-hidden-var']:
    cases[name]=(root/'bind-scope-8daff86'/(name+'.rq')).read_text()
cases['constant-bind-conflict']=(root/'bind-scope-4e54448/constant-bind-conflict.rq').read_text()
cases.update({
 'bind-equal':'SELECT ?x WHERE { VALUES ?x {1} { BIND(1 AS ?x) } }',
 'bind-error':'SELECT ?x WHERE { VALUES ?x {1} { BIND(1/0 AS ?x) } }',
 'bind-datatype-conflict':'SELECT ?x WHERE { VALUES ?x {"1"^^<http://www.w3.org/2001/XMLSchema#int>} { BIND(1 AS ?x) } }',
 'bind-lexical-conflict':'SELECT ?x WHERE { VALUES ?x {"01"^^<http://www.w3.org/2001/XMLSchema#int>} { BIND("1"^^<http://www.w3.org/2001/XMLSchema#int> AS ?x) } }',
 'bind-language-case':'SELECT ?x WHERE { VALUES ?x {"a"@EN} { BIND("a"@en AS ?x) } }',
 'bind-outer-read':'SELECT ?x ?y WHERE { VALUES ?x {1} { BIND(?x+1 AS ?y) } }',
 'optional-conflict':'SELECT ?x ?y WHERE { VALUES ?x {1} OPTIONAL { BIND(2 AS ?x) BIND(9 AS ?y) } }',
 'optional-error':'SELECT ?x ?y WHERE { VALUES ?x {1} OPTIONAL { BIND(1/0 AS ?x) BIND(9 AS ?y) } }',
 'exists-direct':'SELECT ?x WHERE { VALUES ?x {1} FILTER EXISTS { BIND(?x+1 AS ?y) FILTER(?y=2) } }',
 'exists-nested-group':'SELECT ?x WHERE { VALUES ?x {1} FILTER EXISTS { VALUES ?z {0} { BIND(?x+1 AS ?y) } FILTER(?y=2) } }',
 'subquery-limit':'SELECT ?x ?y WHERE { VALUES ?x {2} { SELECT ?x ?y WHERE { VALUES (?x ?y) {(1 10)(2 20)} } ORDER BY ?x LIMIT 1 } }',
 'subquery-offset':'SELECT ?x ?y WHERE { VALUES ?x {2} { SELECT ?x ?y WHERE { VALUES (?x ?y) {(1 10)(2 20)} } ORDER BY ?x LIMIT 1 OFFSET 1 } }',
 'base-parent':'BASE <http://example.test/a/b/> SELECT (IRI("../c") AS ?v) WHERE {}',
 'base-root':'BASE <http://example.test/a/b/> SELECT (URI("/root?x=1#frag") AS ?v) WHERE {}',
 'base-fragment':'BASE <http://example.test/a/b/> SELECT (IRI("#frag") AS ?v) WHERE {}',
 'base-absolute-string':'BASE <http://example.test/a/b/> SELECT (IRI("urn:already") AS ?v) WHERE {}',
 'base-named-iri':'BASE <http://example.test/a/b/> SELECT (IRI(<urn:already>) AS ?v) WHERE {}',
 'base-nonstring-error':'BASE <http://example.test/a/b/> SELECT (IRI(7) AS ?v) WHERE {}',
})
def run(cmd):
    p=subprocess.Popen(cmd,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env={**os.environ,'JAVA_TOOL_OPTIONS':'-Xmx512m -XX:ActiveProcessorCount=1','RAYON_NUM_THREADS':'1'},start_new_session=True)
    try:stdout,stderr=p.communicate(timeout=45)
    except BaseException:
        os.killpg(p.pid,signal.SIGTERM)
        try:p.communicate(timeout=5)
        except subprocess.TimeoutExpired:os.killpg(p.pid,signal.SIGKILL);p.communicate()
        raise
    return p.returncode,stdout,stderr
def bag(result):
    return collections.Counter(tuple((k,exact_term(v)) for k,v in sorted(row.items())) for row in result['results']['bindings'])
(stage/'controls').mkdir()
(stage/'control-manifest.json').write_text(json.dumps({name:hashlib.sha256(q.encode()).hexdigest() for name,q in cases.items()},indent=2))
commands={};expected={}
for name,q in cases.items():
    query=stage/'controls'/(name+'.rq');query.write_text(q)
    cmd=[str(jena/'bin/arq'),'--data='+str(stage/'fixture.nt'),'--query='+str(query),'--results=JSON']
    rc,raw,stderr=run(cmd)
    (stage/'controls'/(name+'-jena.json')).write_bytes(raw);(stage/'controls'/(name+'-jena.stderr')).write_bytes(stderr)
    commands[name]={'jena_command':cmd,'jena_exit':rc}
    (stage/'control-commands.json').write_text(json.dumps(commands,indent=2))
    assert rc==0,(name,stderr)
    expected[name]=json.loads(raw)
results={}
for name,q in cases.items():
    cmd=[str(stage/'rdfdb-311cc51'),'query',str(stage/'fixture.nt'),q]
    rc,raw,stderr=run(cmd)
    (stage/'controls'/(name+'-candidate.json')).write_bytes(raw);(stage/'controls'/(name+'-candidate.stderr')).write_bytes(stderr)
    commands[name].update(candidate_command=cmd,candidate_exit=rc)
    (stage/'control-commands.json').write_text(json.dumps(commands,indent=2))
    assert rc==0,(name,stderr)
    actual=json.loads(raw)
    results[name]={'pass':set(actual['head']['vars'])==set(expected[name]['head']['vars']) and bag(actual)==bag(expected[name]),
       'actual_rows':len(actual['results']['bindings']),'reference_rows':len(expected[name]['results']['bindings'])}
    (stage/'control-results.json').write_text(json.dumps(results,indent=2))
print('passing',sum(r['pass'] for r in results.values()),'of',len(results),flush=True)
assert all(r['pass'] for r in results.values()),[(name,r) for name,r in results.items() if not r['pass']]
