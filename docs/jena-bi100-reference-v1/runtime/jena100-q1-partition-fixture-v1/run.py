import json,hashlib,os,subprocess,sys
from pathlib import Path
stage=Path(__file__).resolve().parent
sys.path.insert(0,str(stage/'code'))
from q1_partition_reference import partition_query,assemble
owner='Jianlin-Su task25 partitioned q1 reference fixture'
assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
jena=Path('<agent-workspace>/rdfdb/bench/tools/apache-jena-6.2.0')
commands=[]
def execute(name,text):
    query=stage/(name+'.rq');query.write_text(text)
    cmd=[str(jena/'bin/arq'),'--data='+str(stage/'data.nt'),'--query='+str(query),'--results=JSON']
    result=subprocess.run(cmd,capture_output=True,timeout=45,env={**os.environ,'JAVA_TOOL_OPTIONS':'-Xmx512m -XX:ActiveProcessorCount=1'})
    (stage/(name+'.stderr')).write_bytes(result.stderr);(stage/(name+'.json')).write_bytes(result.stdout)
    commands.append({'command':cmd,'exit_code':result.returncode,'query_sha256':hashlib.sha256(query.read_bytes()).hexdigest()})
    if result.returncode:raise RuntimeError('Jena fixture request failed: '+name)
    return json.loads(result.stdout)
expected=json.loads((stage/'expected.json').read_text());source=(stage/'original.rq').read_text()
status={'complete':False}
try:
    full=execute('full',source)
    assert [(r['productType']['value'],int(r['reviewCount']['value'])) for r in full['results']['bindings']]==[tuple(v) for v in expected['top10']]
    enumeration=execute('types','PREFIX bsbm: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/> SELECT DISTINCT ?productType WHERE { ?productType a bsbm:ProductType } ORDER BY ?productType')
    types=[r['productType']['value'] for r in enumeration['results']['bindings']];assert types==expected['types']
    status['batch_sizes']={}
    for size in [1,7,8]:
        pieces=[]
        for start in range(0,len(types),size):
            chunk=types[start:start+size];answer=execute(f'b{size}-p{start}',partition_query(source,chunk));pieces.append((chunk,answer))
        actual=assemble(pieces,types)
        assert actual['results']['bindings']==full['results']['bindings']
        (stage/f'assembled-{size}.json').write_text(json.dumps(actual,indent=2))
        status['batch_sizes'][str(size)]={'exact_bag_and_sequence_pass':True}
    status['complete']=True
finally:
    (stage/'commands.json').write_text(json.dumps(commands,indent=2))
    status['data_sha256']=hashlib.sha256((stage/'data.nt').read_bytes()).hexdigest()
    status['source_sha256']={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in (stage/'code').glob('*.py')}
    status['jena_identity']={'arq_script_sha256':hashlib.sha256((jena/'bin/arq').read_bytes()).hexdigest(),'jars':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for pat in ['jena-arq*.jar','jena-core*.jar'] for p in (jena/'lib').glob(pat)}}
    (stage/'status.json').write_text(json.dumps(status,indent=2))
    lock=Path('/tmp/rdf-bench.lock')
    if (lock/'owner').read_text().strip()==owner:(lock/'owner').unlink();lock.rmdir()
