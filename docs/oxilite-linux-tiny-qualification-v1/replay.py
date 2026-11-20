#!/usr/bin/env python3
"""Verify portable evidence and recompute native-answer verdicts; execute no archived code."""
import hashlib,json,tarfile,tempfile
from pathlib import Path
from collections import Counter
ROOT=Path(__file__).resolve().parent
X='http://www.w3.org/2001/XMLSchema#'
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def key(t):
 if t is None:return None
 if t['type']!='literal':return(t['type'],t['value'])
 lang=t.get('xml:lang','').lower();return('literal',t['value'],t.get('datatype') or ('http://www.w3.org/1999/02/22-rdf-syntax-ns#langString' if lang else X+'string'),lang)
def bag(a):
 cols=a['head']['vars'];assert type(cols) is list and len(cols)==len(set(cols))
 for row in a['results']['bindings']:assert set(row)<=set(cols)
 return Counter(tuple(key(row.get(v)) for v in cols) for row in a['results']['bindings'])
def main():
 manifest=json.loads((ROOT/'PACKET_FILES.json').read_text());assert len(manifest)==65
 with tempfile.TemporaryDirectory(prefix='oxilite39-linux-replay-') as d:
  r=Path(d)
  with tarfile.open(ROOT/'packet.tar.gz') as t:
   members=t.getmembers();assert all(m.isfile() for m in members);assert len(set(t.getnames()))==len(members);t.extractall(r,filter='data')
  assert {str(p.relative_to(r)) for p in r.rglob('*') if p.is_file()}==set(manifest)
  for n,h in manifest.items():assert sha(r/n)==h,n
  assert json.loads((r/'STORE_FILE.json').read_text())['logical_bytes']==118784
  s=r/'source';assert sha(s/'FREEZE.json')=='a3481b70e1d30d49093ec9920c42f78a2656127a2cfe5967efcc44f3bb304c0d'
  for n,h in json.loads((s/'FREEZE.json').read_text())['files'].items():assert sha(s/n)==h
  assert sha(r/'TINY_EXECUTION.json')=='a2484fe472a3e8846aa41953cfbd5e3044268394aaf3601b6b19f27e7da44792'
  e=json.loads((r/'TINY_EXECUTION.json').read_text());assert sha(r/'runtime/RUNTIME.json')==e['runtime_sha256'];assert sha(r/'setup/RECEIPT.json')==e['setup_receipt_sha256']
  runtime=json.loads((r/'runtime/RUNTIME.json').read_text());assert runtime['python']=='3.11.18' and runtime['machine']=='aarch64' and runtime['pip_version']=='24.0' and runtime['distribution_version']=='0.10.0';assert len(runtime['files'])==18 and runtime['record_entries_verified']==18;assert runtime['engine_imported'] is False and runtime['queries_executed'] is False
  for stage in ['setup','tiny']:
   rc=json.loads((r/stage/'RECEIPT.json').read_text());assert rc['error'] is None and rc['cleanup']=='verified-removed' and rc['source_and_lock_postcheck'] is True;assert rc['exit_code']==0 and rc['attach_exit']==0 and rc['oom_killed'] is False and rc['running'] is False;assert all(v is True for v in rc['inspect']['checks'].values());assert rc['image_id']==e['image_id']
   post=json.loads((r/(stage.upper()+'_POSTFLIGHT.json')).read_text());assert post['returncode']==0 and post['child_waited'] is True and post['outer_timeout'] is False and post['lock_released'] is True
  ref=json.loads((s/'consumer-fixture-jena.json').read_text());plain={'head':{'vars':['o']},'results':{'bindings':[{'o':row['o']} for row in ref['results']['bindings']]}}
  count={'head':{'vars':['n']},'results':{'bindings':[{'n':{'type':'literal','value':'9','datatype':X+'integer'}}]}}
  expected={'group':ref,'plain':plain,'distinct':plain,'count':count,'distinct_count':count,'empty':{'head':{'vars':['o']},'results':{'bindings':[]}},'unbound':{'head':{'vars':['o','missing']},'results':plain['results']},'ask_false':{'boolean':False}};verdicts={}
  profiles=json.loads((r/'tiny/out/PROFILES.json').read_text());assert profiles==[{'mode':mode,'exit_code':0} for mode in ['memory','disk-load','disk-reopen']]
  for mode in ['memory','disk-load','disk-reopen']:
   out=r/'tiny/out'/mode;summary=json.loads((out/'summary.json').read_text());assert summary['mode']==mode and type(summary['stored_quads']) is int and summary['stored_quads']==9 and summary['scored'] is False and summary['full_workload_qualified'] is False
   identity=json.loads((out/'identity.json').read_text());assert identity['executable_sha256']==runtime['executable_sha256'] and identity['version']=='0.10.0';assert {'site/'+n:h for n,h in identity['distribution_files'].items()}==runtime['files']
   for label,want in expected.items():
    record=json.loads((out/(label+'.json')).read_text());assert 'error' not in record and 'explain_error' not in record;a=record['answer']
    if label=='ask_false':ok=type(a.get('boolean')) is bool and a==want
    else:ok=a['head']['vars']==want['head']['vars'] and bag(a)==bag(want)
    if label=='unbound':ok=ok and all('missing' not in row for row in a['results']['bindings'])
    assert ok,(mode,label);verdicts[mode+'/'+label]=ok
  rc=json.loads((r/'tiny/RECEIPT.json').read_text());assert rc['answers']['tiny_admitted'] is True and rc['answers']['full_workload_qualified'] is False and rc['answers']['scored'] is False
  print(json.dumps({'files_verified':len(manifest),'native_answers_replayed':len(verdicts),'profiles':['memory','disk-load','disk-reopen'],'all_pass':all(verdicts.values()),'engine_executed':False,'full_workload_qualified':False,'binaries_reconstructed':False}))
if __name__=='__main__':main()
