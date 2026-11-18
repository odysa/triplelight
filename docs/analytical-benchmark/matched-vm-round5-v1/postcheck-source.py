from pathlib import Path
import json,hashlib,sys,subprocess
root=Path(__file__).parent;s=root/'matched-vm-10m';run=s/'round5-main1cd8881-three'
repo=Path('<agent-workspace>/rdfdb')
sys.path.insert(0,str(repo/'bench/analytical'))
from summarize_value_http import audit
from envelope_qualify import FrozenInputs
from count_equivalence import load_freeze
owner='Jianlin-Su task25 BI matched-VM main1cd8881 round5'
assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()==owner
sha=lambda p:hashlib.file_digest(p.open('rb'),'sha256').hexdigest()
policy=s/'value-policy-freeze.json';digest='70ad4ab0cc3299c20a8177c64568d0698bbe8eed3b391d122214d94ebf1f0f6a'
summary=audit(run,root/'bsbm10-holdout-v1/jena',root/'numeric-diagnostics-10m',policy,digest)
(run/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
assert summary['complete_value_compatible_three_engine_ranking']
checks={}
identity=json.loads((s/'input-identity.json').read_text());checks['source_input']={'sha256':sha(Path(identity['source']))};assert checks['source_input']['sha256']==identity['sha256']
for name,file,expected in [
 ('duckdb_readonly_store',s/'duck.db','f86cb05c12f8cd1dbe7d62f5b918b03ea95e945435a7148f71d1255927dba421'),
 ('rdfdb1cd_binary',s/'target-linux-1cd8881/release/rdfdb',json.loads((s/'rdfdb-binary-1cd8881.json').read_text())['sha256']),
 ('prior20b_binary',s/'target-linux-20b257a/release/rdfdb','b302f11c49cc2e24d6d2a23d1b7cb3c51f2c358f5f17ee49c5395d2cfa3579d4'),
 ('prior738_binary',s/'target-linux-738f0cb/release/rdfdb','8277d341b2a514a3405728530c2bb0de176b3b8b48f11bcbc3f74d4c2b477d23'),
 ('prior71_binary',s/'target-linux/release/rdfdb','4e9e3e11b7d6fb530856ec39ff41430c86123f4412803d6ed38b46fc97f26352')]:
  got=sha(file);assert got==expected;checks[name]={'sha256':got,'matches':True}
metadata=json.loads((s/'rdfdb-source-1cd8881.json').read_text())
for name,h in metadata['files'].items():assert sha(s/'rdfdb-source-1cd8881'/name)==h
checks['source_files_verified']=len(metadata['files'])
fixture=json.loads((s/'lifetime-fixture-1cd8881/qualification.json').read_text());assert fixture['exact_Jena_bag'] and fixture['rows']==6 and fixture['binary']['sha256']==sha(s/'target-linux-1cd8881/release/rdfdb');checks['new_linux_lifetime_fixture_exact_Jena_bag']=True
work=s/'bench-source/workloads/bsbm10';raw=(work/'manifest.json').read_bytes();m=json.loads(raw)
frozen=FrozenInputs(root/'numeric-diagnostics-10m','a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244',m['source_data_sha256'],hashlib.sha256(raw).hexdigest())
for q in m['queries']:
 assert sha(work/'queries'/q['file'])==q['sha256']
 if q['template'] in (4,5,6):frozen.for_instance(q)
checks.update(query_hashes_verified=60,numeric_bindings_verified=32)
count=repo/'docs/analytical-benchmark/count-equivalence-v1/freeze.json';checks['count_freeze']=load_freeze(count,'ffa49a6ce10c19c02d6185a3431383157c590e1e1beb9c9d37b7b83e03bf5790')
for name,entry in summary['profiles'].items():
 phase=run/name;state=json.loads((phase/'container-state.json').read_text());assert not state['Running'] and not state['OOMKilled']
 checks[name+'_state']={'running':state['Running'],'OOMKilled':state['OOMKilled'],'exit':state['ExitCode']}
 if name.endswith('duckdb'):
  native=json.loads((phase/'loaded-duckdb.json').read_text());assert native['version']=='2.0.0' and native['native_sha256']=='4cc47e49dd4bc2817b01645c652d4ba192f863d3e4df2f5b274f530a1adbb7c1'
 if name.endswith('qlever'):
  settings=json.loads((phase/'settings.json').read_text());assert settings['memory-for-queries']=='11 GB' and settings['max-num-threads-per-query']=='4' and settings['disable-caching']=='false' and settings['group-by-hash-map-enabled']=='false'
for p,h in json.loads((root/'round5-preflight-identities.json').read_text()).items():assert sha(Path(p))==h
checks['preflight_identity_files_unchanged']=True
# Inspect only our six exact names, never other agents' process configuration.
for name in summary['profiles']:
 passno,mode=name.split('-',1);container='rdf-jianlin-bi-r5-p'+passno.removeprefix('pass')+'-'+mode
 result=subprocess.run(['docker','inspect',container,'--format','{{.State.Running}}'],text=True,capture_output=True)
 assert result.returncode!=0,'Owned phase container still exists'
checks['all_owned_phase_containers_removed']=True
from collections import Counter
from qualify import exact_term, ORDER_KEYS
host=root/'probe-set-1cd8881/queries'
host_records={r['path']:r for r in json.loads((repo/'docs/analytical-benchmark/probe-set-qualification-v1/file-manifest.json').read_text())}
def term_rows(doc,variables):
 return [tuple(exact_term(row.get(v)) for v in variables) for row in doc['results']['bindings']]
for phase_name in ('pass1-rdfdb','pass2-rdfdb'):
 bags=seqs=seq_total=0
 for query in m['queries']:
  stem=Path(query['file']).stem
  assert sha(host/(stem+'.body'))==host_records['queries/'+stem+'.body']['sha256']
  actual=json.loads((run/phase_name/'queries'/(stem+'.body')).read_bytes());reference=json.loads((host/(stem+'.body')).read_bytes())
  a,b=term_rows(actual,reference['head']['vars']),term_rows(reference,reference['head']['vars'])
  bags+=Counter(a)==Counter(b)
  if query['template'] in ORDER_KEYS:seq_total+=1;seqs+=a==b
 checks[phase_name+'_host_equivalence']={'exact_bags':bags,'bags':60,'exact_sequences':seqs,'sequences':seq_total,'scope':'Additional host/VM comparison; does not change value gate'}

(run/'postchecks.json').write_text(json.dumps(checks,indent=2)+'\n')
print(json.dumps({name:v['value_compatible_total_ms'] for name,v in summary['profiles'].items()},indent=2))
print('POSTCHECKS PASS; caller may release own lock')
