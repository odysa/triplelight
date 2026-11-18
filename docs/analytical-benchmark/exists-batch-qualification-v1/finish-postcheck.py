from pathlib import Path
import argparse,hashlib,json,sys
parser=argparse.ArgumentParser();parser.add_argument('--stage',required=True);parser.add_argument('--owner',required=True);a=parser.parse_args()
r=Path(__file__).parent;p=r/a.stage;repo=Path('<agent-workspace>/rdfdb')
sha=lambda f:hashlib.sha256(f.read_bytes()).hexdigest()
status=json.loads((p/'status.json').read_text());assert status['client_exit']==0 and status['server_exit']==-15 and status['store_identity_unchanged']
source=json.loads((p/'source.json').read_text())
for n,h in source['files'].items():assert sha(p/'source'/n)==h
for n,h in json.loads((p/'validator-sources.json').read_text()).items():assert sha(repo/'bench/analytical'/n)==h
assert sha(p/'rdfdb')==status['binary_posthash']==json.loads((p/'binary.json').read_text())['sha256']
m=json.loads((p/'queries/measurements.json').read_text());assert m['complete'] and len(m['queries'])==60 and all(v['status']=='pass' for v in m['queries'].values())
c=json.loads((p/'baseline-comparison.json').read_text());assert len(c)==60 and all(v['exact_term_bag'] for v in c.values());seq=[v for v in c.values() if v['ordered_sequence'] is not None];assert len(seq)==36 and all(v['ordered_sequence'] for v in seq)
archive={v['path']:v for v in json.loads((repo/'docs/analytical-benchmark/matched-vm-v1/file-manifest.json').read_text())};refs={v['path']:v for v in json.loads((repo/'docs/analytical-benchmark/holdout-v1/file-manifest.json').read_text())}
for n,v in m['queries'].items():
 stem=Path(n).stem;assert sha(p/'queries'/(stem+'.body'))==v['body_sha256']
 assert sha(r/'matched-vm-10m/round1-retry1/pass1-rdfdb/queries'/(stem+'.body'))==archive['round/pass1-rdfdb/queries/'+stem+'.body']['sha256']
 assert sha(r/'bsbm10-holdout-v1/jena'/(stem+'.json'))==refs['jena/'+stem+'.json']['sha256']
sys.path.insert(0,str(repo/'bench/analytical'));from envelope_qualify import FrozenInputs
work=repo/'bench/analytical/workloads/bsbm10';manifest=json.loads((work/'manifest.json').read_text());frozen=FrozenInputs(r/'numeric-diagnostics-10m','a9dd62b09ae40919c14bd28bd626ef8962190f1854b817b09b6a2ce6f9c11244',manifest['source_data_sha256'],sha(work/'manifest.json'))
for q in manifest['queries']:
 assert sha(work/'queries'/q['file'])==q['sha256']
 if q['template'] in (4,5,6):frozen.for_instance(q)
record={'candidate':source['commit'],'frozen_oracle_pass':60,'exact_term_bags_pass':60,'ordered_sequences_pass':36,'source_files_postverified':len(source['files']),'body_hashes_verified':60,'baseline_and_Jena_published_hashes_verified':60,'query_hashes_verified':60,'numeric_bindings_verified':32,'numeric_freeze_sha256':frozen.freeze_sha256,'validator_sources_postverified':True,'candidate_binary_postverified':True,'server_exit':status['server_exit'],'store_manifest_unchanged':True,'performance_AB_claim':False}
(p/'frozen-postcheck.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record,indent=2))
lock=Path('/tmp/rdf-bench.lock/owner');assert lock.read_text().strip()==a.owner;lock.unlink();lock.parent.rmdir();print('OWN LOCK RELEASED')
