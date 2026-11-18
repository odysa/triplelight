import hashlib,json,sys
from pathlib import Path
root=Path(__file__).resolve().parent;stage=root/'jena100-q1-partition-v1'
sys.path.insert(0,str(root/'jena100-q1-partition-code-v1'))
from q1_partition_reference import partition_query,assemble
state=json.loads((stage/'status.json').read_text());assert state['complete'] and state['only_expected_lock_metadata_changed'] and state['nt_stat_unchanged'] and state['own_lock_released']
assert state['server_exit']==143
matrix=json.loads((stage/'reference/matrix.json').read_text());assert matrix['complete'] and matrix['all_requested_pass']
domain=json.loads((stage/'reference/domain.json').read_text());types=[r['productType']['value'] for r in domain['results']['bindings']]
assert len(types)==2011 and len(types)==len(set(types)) and all(r['productType']['type']=='uri' for r in domain['results']['bindings'])
work=root/'bsbm100-preparation-v1/workload';manifest=json.loads((work/'manifest.json').read_text());records={q['file']:q for q in manifest['queries']}
checks={}
for name,entry in matrix['queries'].items():
    case=stage/'reference'/Path(name).stem;source=(work/'queries'/name).read_text()
    assert hashlib.sha256(source.encode()).hexdigest()==entry['original_query_sha256']==records[name]['sha256']
    pieces=[]
    for start in range(0,len(types),8):
        selected=types[start:start+8];stem=f'partition-{start//8:04}'
        assert (case/(stem+'.rq')).read_text()==partition_query(source,selected)
        answer=json.loads((case/(stem+'.json')).read_text());pieces.append((selected,answer))
    actual=assemble(pieces,types)
    saved=json.loads((stage/'reference'/(Path(name).stem+'.json')).read_text());assert actual==saved
    assert entry['partitions_complete']==len(pieces)==252
    if name!='m002_p00_q1.rq':assert entry['exact_full_query_overlap_pass']
    checks[name]={'query_and_all_partition_sources_pass':True,'reassembled_exactly':True,'partitions':len(pieces)}
for path,old in [(root/'jena100-q1-partition-run-source-sha256.json',False),(root/'jena100-bi-code-v1/source-sha256.json',True)]:
    for name,digest in json.loads(path.read_text()).items():
        source=(root/'jena100-bi-code-v1'/name) if old else root/name
        assert hashlib.sha256(source.read_bytes()).hexdigest()==digest
initial=json.loads((root/'jena100-count-v1/store-after.json').read_text());now=json.loads((stage/'store-after.json').read_text())
assert {k:v for k,v in initial.items() if not k.endswith('tdb.lock')}=={k:v for k,v in now.items() if not k.endswith('tdb.lock')}
result={'complete':True,'domain_size':len(types),'queries':checks,'partition_queries_verified':1008,'initial_shared_store_data_metadata_unchanged':True,'no_competitor_outputs_used':True}
(root/'jena100-q1-partition-postchecks.json').write_text(json.dumps(result,indent=2));print(json.dumps(result))
