import collections,hashlib,json,os,signal,subprocess,sys
from pathlib import Path
stage=Path(__file__).resolve().parent;root=stage.parent
sys.path.insert(0,str(root/'w3c-string-ef7a778/bench-source'))
from qualify import exact_term
jena=Path('<agent-workspace>/rdfdb/bench/tools/apache-jena-6.2.0')
prefix='PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> '
cases={
 'add':'SELECT ((0.1+0.2) AS ?v) WHERE {}',
 'sum':'SELECT (SUM(?x) AS ?v) WHERE {VALUES ?x {1.1 10.0}}',
 'avg':'SELECT (AVG(?x) AS ?v) WHERE {VALUES ?x {1.1 10.0}}',
 'quarter':'SELECT ((1/4) AS ?v) WHERE {}',
 'integer-zero':'SELECT ((1/0) AS ?v) WHERE {}',
 'decimal-zero':'SELECT ((1.0/0.0) AS ?v) WHERE {}',
 'double-zero':'SELECT ((1e0/0e0) AS ?v) WHERE {}',
 'negative-round':'SELECT (ROUND(-1.5) AS ?v) (ROUND(-2.5) AS ?w) WHERE {}',
 'ceil-floor':'SELECT (CEIL(9007199254740992.1) AS ?v) (FLOOR(-9007199254740992.1) AS ?w) WHERE {}',
 'cast':'SELECT (xsd:decimal("9007199254740992.1") AS ?v) WHERE {}',
 'compiled-filter':'SELECT ?x WHERE {VALUES ?x {0.3} FILTER(?x=0.1+0.2)}',
 'decimal-compare':'SELECT ((9007199254740992.1 < 9007199254740992.2) AS ?v) WHERE {}',
 'precision-28':'SELECT ((0.1000000000000000000000000001+0.2) AS ?v) WHERE {}',
 'coefficient-overflow':'SELECT ((99999999999999999999999999999.0+0.0) AS ?v) WHERE {}',
 'scale-29':'SELECT ((0.00000000000000000000000000001+0.0) AS ?v) WHERE {}',
 'integer-overflow':'SELECT ((9223372036854775807+1) AS ?v) WHERE {}',
 'repeating-division':'SELECT ((1/3) AS ?v) WHERE {}',
 'mixed-float':'SELECT ((xsd:float("0.1")+0.2) AS ?v) WHERE {}',
 'decimal-max-round':'SELECT (ROUND(79228162514264337593543950335.0) AS ?v) WHERE {}',
 'decimal-overflow':'SELECT ((79228162514264337593543950335.0+1.0) AS ?v) WHERE {}',
 'beyond-decimal-coefficient':'SELECT ((79228162514264337593543950336.0+0.0) AS ?v) WHERE {}',
 'sub-scale-29':'SELECT ((0.00000000000000000000000000001+0.0) AS ?v) WHERE {}',
 'float-promotion-add':'SELECT (("16777217"^^xsd:decimal + "-16777216"^^xsd:float) AS ?v) WHERE {}',
 'float-promotion-compare':'SELECT (("16777217"^^xsd:decimal = "16777216"^^xsd:float) AS ?v) WHERE {}',
 'mixed-double':'SELECT ((xsd:double("0.1")+0.2) AS ?v) WHERE {}',
}
cases={k:prefix+q for k,q in cases.items()}
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
(stage/'controls-v2').mkdir()
(stage/'control-manifest-v2.json').write_text(json.dumps({name:hashlib.sha256(q.encode()).hexdigest() for name,q in cases.items()},indent=2))
commands={};expected={}
for name,q in cases.items():
    query=stage/'controls-v2'/(name+'.rq');query.write_text(q)
    cmd=[str(jena/'bin/arq'),'--data='+str(stage/'fixture.nt'),'--query='+str(query),'--results=JSON']
    rc,raw,stderr=run(cmd)
    (stage/'controls-v2'/(name+'-jena.json')).write_bytes(raw);(stage/'controls-v2'/(name+'-jena.stderr')).write_bytes(stderr)
    commands[name]={'jena_command':cmd,'jena_exit':rc}
    (stage/'control-commands-v2.json').write_text(json.dumps(commands,indent=2))
    if rc==0:expected[name]=json.loads(raw)
results={}
for name,q in cases.items():
    cmd=[str(stage/'rdfdb-09e1004'),'query',str(stage/'fixture.nt'),q]
    rc,raw,stderr=run(cmd)
    (stage/'controls-v2'/(name+'-candidate.json')).write_bytes(raw);(stage/'controls-v2'/(name+'-candidate.stderr')).write_bytes(stderr)
    commands[name].update(candidate_command=cmd,candidate_exit=rc)
    (stage/'control-commands-v2.json').write_text(json.dumps(commands,indent=2))
    if rc != 0:
        results[name]={'pass':False,'candidate_exit':rc,'error':'Candidate process failed; raw stderr retained'}
        (stage/'control-results-v2.json').write_text(json.dumps(results,indent=2));continue
    actual=json.loads(raw)
    if name not in expected:
        results[name]={'pass':None,'reference_error':True,'actual_rows':len(actual['results']['bindings'])}
        (stage/'control-results-v2.json').write_text(json.dumps(results,indent=2));continue
    results[name]={'pass':set(actual['head']['vars'])==set(expected[name]['head']['vars']) and bag(actual)==bag(expected[name]),
       'actual_rows':len(actual['results']['bindings']),'reference_rows':len(expected[name]['results']['bindings'])}
    (stage/'control-results-v2.json').write_text(json.dumps(results,indent=2))
print('passing',sum(r['pass'] is True for r in results.values()),'of',len(results),flush=True)
# Preserve every exact-term difference for independent review; no frozen BI policy changes.

