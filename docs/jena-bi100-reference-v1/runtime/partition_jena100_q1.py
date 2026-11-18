import hashlib,json,sys
from pathlib import Path
root=Path(__file__).resolve().parent
sys.path[:0]=[str(root/'jena100-q1-partition-code-v1'),str(root/'jena100-bi-code-v1')]
from q1_partition_reference import partition_query,assemble
from workload_check import ask
from qualify import qualify,check_order
from supervise_import import snapshot
owner='Jianlin-Su task25 partitioned Jena q1 reference'

def collect(endpoint,out):
    fixture=root/'jena100-q1-partition-fixture-v1'
    status=json.loads((fixture/'status.json').read_text())
    assert status['complete'] and json.loads((fixture/'unit-tests.json').read_text())['exit_code']==0
    helper=root/'jena100-q1-partition-code-v1/q1_partition_reference.py'
    assert hashlib.sha256(helper.read_bytes()).hexdigest()==status['source_sha256']['q1_partition_reference.py']
    workload=root/'bsbm100-preparation-v1/workload'
    manifest_raw=(workload/'manifest.json').read_bytes();manifest=json.loads(manifest_raw)
    assert hashlib.sha256(manifest_raw).hexdigest()=='a6d27c91ec733626fa0d132f642d5a348c62cbfc00b423809bf82074cb1a35ef'
    out.mkdir()
    domain_query='PREFIX bsbm: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/> SELECT DISTINCT ?productType WHERE { ?productType a bsbm:ProductType } ORDER BY ?productType'
    (out/'domain.rq').write_text(domain_query)
    raw=ask(endpoint,domain_query,120);(out/'domain.json').write_text(json.dumps(raw,indent=2))
    assert raw['head']['vars']==['productType']
    assert all(row['productType']['type']=='uri' for row in raw['results']['bindings'])
    types=[row['productType']['value'] for row in raw['results']['bindings']]
    assert len(types)==len(set(types)) and types==sorted(types)
    report={'schema':'partitioned-jena-q1-reference-v1','domain_size':len(types),
      'partition_size':8,'queries':{},'complete':False,'original_queries_completed':False,
      'reference_label':'Jena group-key partitioned execution, unchanged grouped COUNT semantics'}
    records={q['file']:q for q in manifest['queries']}
    for name in ['m000_p00_q1.rq','m001_p00_q1.rq','m003_p00_q1.rq','m002_p00_q1.rq']:
        q=records[name];source=(workload/'queries'/name).read_text()
        assert hashlib.sha256(source.encode()).hexdigest()==q['sha256']
        case=out/Path(name).stem;case.mkdir();results=[]
        entry={'original_query_sha256':q['sha256'],'partitions_complete':0,'status':'running'}
        report['queries'][name]=entry
        for start in range(0,len(types),8):
            assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
            guard=snapshot(root,owner)
            if guard['abort']:raise RuntimeError(guard['abort'])
            selected=types[start:start+8];text=partition_query(source,selected)
            stem=f'partition-{start//8:04}'
            (case/(stem+'.rq')).write_text(text)
            answer=ask(endpoint,text,120);(case/(stem+'.json')).write_text(json.dumps(answer,indent=2))
            results.append((selected,answer));entry['partitions_complete']+=1
            (out/'matrix.json').write_text(json.dumps(report,indent=2))
        actual=assemble(results,types);(out/(Path(name).stem+'.json')).write_text(json.dumps(actual,indent=2))
        entry.update(status='pass',order=check_order(1,actual),rows=len(actual['results']['bindings']))
        if name!='m002_p00_q1.rq':
            prior=root/('jena100-bi-retry-v1' if name.startswith('m003') else 'jena100-bi-reference-v1')/'reference'/(Path(name).stem+'.json')
            expected=json.loads(prior.read_text());qualify(1,actual,expected)
            assert actual['results']['bindings']==expected['results']['bindings']
            entry['exact_full_query_overlap_pass']=True
            entry['overlap_reference_sha256']=hashlib.sha256(prior.read_bytes()).hexdigest()
        (out/'matrix.json').write_text(json.dumps(report,indent=2))
    report['complete']=True;report['all_requested_pass']=True
    (out/'matrix.json').write_text(json.dumps(report,indent=2))
    return report
