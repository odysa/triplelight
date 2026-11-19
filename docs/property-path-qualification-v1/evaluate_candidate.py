import argparse,hashlib,json,os
from pathlib import Path
from qualify_paths import sha,canonical,compare,invoke
p=Path(__file__).resolve().parent
ap=argparse.ArgumentParser();ap.add_argument('--suite',default='suite.json');ap.add_argument('--references',default='oracle-attempt1');ap.add_argument('--output',required=True);ap.add_argument('--adjudication');args=ap.parse_args()
suite_path=p/args.suite;suite=json.loads(suite_path.read_text());refdir=p/args.references;refs=json.loads((refdir/'identity.json').read_text());assert sha(suite_path)==refs['suite_sha256']
policy=json.loads((p/args.adjudication).read_text()) if args.adjudication else None
if policy:assert policy['suite_sha256']==sha(suite_path)
anchors=policy['normative_boolean_anchors'] if policy else {}
binary=p/'triplelight-52fd1d71';build=json.loads((p/'build-status.json').read_text());assert build['exit_code']==0 and sha(binary)==build['binary_sha256']
out=p/args.output;out.mkdir(exist_ok=False);records={}
identity={'suite_sha256':sha(suite_path),'binary_sha256':sha(binary),'source_commit':build['source_commit'],'adjudication_sha256':sha(p/args.adjudication) if policy else None,'reference_identity_sha256':sha(refdir/'identity.json')};(out/'identity.json').write_text(json.dumps(identity,indent=2)+'\n')
for c in suite['cases']:
 name=c['name'];assert sha(p/c['query'])==c['query_sha256'];assert sha(p/c['fixture'])==suite['fixtures'][c['fixture']]
 ref=refdir/(name+'-jena.json');assert sha(ref)==refs['references'][name];expected=json.loads(ref.read_text());canonical(expected)
 if name in anchors:assert c['expected_boolean'] is anchors[name] and expected['boolean'] is not anchors[name]
 cmd=[str(binary),'query',str(p/c['fixture']),(p/c['query']).read_text()];actual=out/(name+'-candidate.json');record={'process':invoke(cmd,actual,out/(name+'-candidate.stderr'),45,{**os.environ,'RAYON_NUM_THREADS':'1'}),'jena_exact_pass':False,'contract_pass':False,'oracle_disagreement':name in anchors};records[name]=record
 try:
  assert record['process']['exit_code']==0,'candidate process failed';got=json.loads(actual.read_text());record['jena_exact_pass']=compare(got,expected,c['ordered']);record['contract_pass']=compare(got,{'boolean':anchors[name]}) if name in anchors else record['jena_exact_pass'];record['candidate_rows']=len(got['results']['bindings']) if 'results' in got else None
 except Exception as error:record['error']=repr(error)
 (out/'results.json').write_text(json.dumps(records,indent=2)+'\n')
assert sha(binary)==identity['binary_sha256'] and sha(suite_path)==identity['suite_sha256']
for rel,digest in refs['inputs'].items():assert sha(p/rel)==digest
for name,digest in refs['references'].items():assert sha(refdir/(name+'-jena.json'))==digest
summary={'complete':True,'cases':len(records),'jena_exact_pass':sum(r['jena_exact_pass'] for r in records.values()),'declared_contract_pass':sum(r['contract_pass'] for r in records.values()),'oracle_disagreements':list(anchors),'contract_gate':all(r['contract_pass'] for r in records.values()),'inputs_and_binary_unchanged':True}
(out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n');print(json.dumps(summary,indent=2));raise SystemExit(0 if summary['contract_gate'] else 1)
