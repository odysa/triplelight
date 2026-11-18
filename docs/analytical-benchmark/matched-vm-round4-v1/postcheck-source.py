from pathlib import Path
import json,hashlib,sys,subprocess
root=Path(__file__).parent;s=root/'matched-vm-10m';run=s/'round4-maincc15f30-three'
repo=Path('<agent-workspace>/rdfdb')
sys.path.insert(0,str(repo/'bench/analytical'))
from summarize_value_http import audit
from envelope_qualify import FrozenInputs
from count_equivalence import load_freeze
owner='Jianlin-Su task25 BI matched-VM three-engine round4'
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
 ('rdfdb20b_binary',s/'target-linux-20b257a/release/rdfdb','b302f11c49cc2e24d6d2a23d1b7cb3c51f2c358f5f17ee49c5395d2cfa3579d4'),
 ('prior738_binary',s/'target-linux-738f0cb/release/rdfdb','8277d341b2a514a3405728530c2bb0de176b3b8b48f11bcbc3f74d4c2b477d23'),
 ('prior71_binary',s/'target-linux/release/rdfdb','4e9e3e11b7d6fb530856ec39ff41430c86123f4412803d6ed38b46fc97f26352')]:
  got=sha(file);assert got==expected;checks[name]={'sha256':got,'matches':True}
metadata=json.loads((s/'rdfdb-source-20b257a.json').read_text())
for name,h in metadata['files'].items():assert sha(s/'rdfdb-source-20b257a'/name)==h
checks['source_files_verified']=len(metadata['files'])
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
for p,h in json.loads((root/'round4-preflight-identities.json').read_text()).items():assert sha(Path(p))==h
checks['preflight_identity_files_unchanged']=True
# Inspect only our six exact names, never other agents' process configuration.
for name in summary['profiles']:
 passno,mode=name.split('-',1);container='rdf-jianlin-bi-r4-p'+passno.removeprefix('pass')+'-'+mode
 result=subprocess.run(['docker','inspect',container,'--format','{{.State.Running}}'],text=True,capture_output=True)
 assert result.returncode!=0,'Owned phase container still exists'
checks['all_owned_phase_containers_removed']=True
(run/'postchecks.json').write_text(json.dumps(checks,indent=2)+'\n')
print(json.dumps({name:v['value_compatible_total_ms'] for name,v in summary['profiles'].items()},indent=2))
print('POSTCHECKS PASS; caller may release own lock')
