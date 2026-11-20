#!/usr/bin/env python3
"""Replay saved answers without engine dependencies or executing archived source."""
import hashlib,json,tarfile,tempfile
from pathlib import Path
from collections import Counter
ROOT=Path(__file__).resolve().parent
X='http://www.w3.org/2001/XMLSchema#'
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def term(t):
 if t is None:return None
 if t['type']!='literal':return(t['type'],t['value'])
 lang=t.get('xml:lang','').lower()
 return ('literal',t['value'],t.get('datatype') or ('http://www.w3.org/1999/02/22-rdf-syntax-ns#langString' if lang else X+'string'),lang)
def bag(a):return Counter(tuple(term(row.get(v)) for v in a['head']['vars']) for row in a['results']['bindings'])
def main():
 manifest=json.loads((ROOT/'PACKET_FILES.json').read_text())
 with tempfile.TemporaryDirectory(prefix='ontop46-replay-') as d:
  r=Path(d)
  with tarfile.open(ROOT/'packet.tar.gz') as t:
   members=t.getmembers();assert all(m.isfile() for m in members);assert len({m.name for m in members})==len(members);t.extractall(r,filter='data')
  assert {str(p.relative_to(r)) for p in r.rglob('*') if p.is_file()}==set(manifest)
  for n,h in manifest.items():assert sha(r/n)==h,n
  s=r/'source';assert sha(s/'FREEZE.json')=='4378cf865ef308885a1ae6b10db29168fc1f9b34cede00336e5ef393513060b2'
  for n,h in json.loads((s/'FREEZE.json').read_text())['files'].items():assert sha(s/n)==h
  assert sha(r/'EXECUTION.json')=='fb36f9ec831ae3aa11749e746bac27eba4e7aebfa01c308a167906ab459cf431'
  e=json.loads((r/'EXECUTION.json').read_text());assert sha(r/'RUNTIME.json')==e['runtime_sha256']
  ref=json.loads((s/'consumer-fixture-jena.json').read_text());objects=[row['o'] for row in ref['results']['bindings']]
  def select(ts):return {'head':{'vars':['o']},'results':{'bindings':[{'o':t} for t in ts]}}
  plain=select(objects);count={'head':{'vars':['n']},'results':{'bindings':[{'n':{'type':'literal','value':'9','datatype':X+'integer'}}]}}
  expected={'group':ref,'plain':plain,'distinct':plain,'count':count,'distinct_count':count,'empty':select([]),'unbound':{'head':{'vars':['o','missing']},'results':plain['results']},'ask_false':{'boolean':False},'same_iri':select([t for t in objects if t['type']=='uri' and t['value']=='urn:iri']),'is_iri':select([t for t in objects if t['type']=='uri']),'same_string':select([t for t in objects if term(t)==('literal','urn:iri',X+'string','')]),'same_lang':select([t for t in objects if t.get('xml:lang','').lower()=='en']),'datatype_int':select([t for t in objects if t.get('datatype')==X+'int'])}
  verdicts={}
  for name,want in expected.items():
   record=json.loads((r/'attempt-1/child'/(name+'.json')).read_text());a=record['answer'];assert 'error' not in record and 'error_class' not in record
   assert record['query']==(s/'queries'/(name+'.rq')).read_text()
   verdicts[name]=(type(a.get('boolean')) is bool and a==want) if name=='ask_false' else (a['head']['vars']==want['head']['vars'] and bag(a)==bag(want))
   if name=='unbound':assert all('missing' not in row for row in a['results']['bindings'])
  assert verdicts=={k:k!='distinct_count' for k in expected}
  dc=json.loads((r/'attempt-1/child/distinct_count.json').read_text())['answer'];assert dc['results']['bindings']==[{'n':{'type':'literal','value':'8','datatype':X+'integer'}}]
  sup=json.loads((r/'attempt-1/supervisor.json').read_text());assert sup['query_checks']['case_verdicts']==verdicts and sup['exit_code']==0 and sup['timeout'] is False and sup['source_runtime_unchanged'] is True and sup['driver_completed'] is True and sup['tiny_answer_admitted'] is False and sup['full_workload_qualified'] is False
  post=json.loads((r/'QUERY_POSTFLIGHT.json').read_text());assert post['returncode']==0 and post['outer_timeout'] is False and post['child_waited'] is True and post['lock_released'] is True
  print(json.dumps({'files_verified':len(manifest),'answers_replayed':13,'pass':12,'fail':['distinct_count'],'observed_distinct_count':8,'expected_distinct_count':9,'engine_executed':False,'full_workload_qualified':False}))
if __name__=='__main__':main()
