"""Portable, independent replay of a Ladybug mapping evidence packet (stdlib only).
Layout: measurement/{frozen,*.record.json,*.gate.json,*.json.gz,supervisor.json}, references/.
Does not load an engine or execute archived drivers.
"""
import sys,json,gzip,hashlib,math,collections,datetime,re
from decimal import Decimal
from pathlib import Path
sys.dont_write_bytecode=True
XSD='http://www.w3.org/2001/XMLSchema#'
def sha(path):return hashlib.sha256(Path(path).read_bytes()).hexdigest()
def canonical(t):
 if t is None:return None
 kind='literal' if t['type']=='typed-literal' else t['type'];value=t['value'];datatype=t.get('datatype');lang=t.get('xml:lang')
 assert kind in ['uri','bnode','literal'] and isinstance(value,str)
 assert datatype is None or isinstance(datatype,str)
 if kind!='literal':assert datatype is None and lang is None
 if lang:
  assert datatype in [None,'http://www.w3.org/1999/02/22-rdf-syntax-ns#langString'];return(kind,value,None,lang.lower())
 return(kind,value,None if datatype==XSD+'string' else datatype,None)
def rows(payload):
 variables=payload['head']['vars'];assert len(variables)==len(set(variables));variables=sorted(variables)
 result=[]
 for binding in payload['results']['bindings']:
  assert set(binding)<=set(variables);result.append(tuple(canonical(binding.get(v)) for v in variables))
 return variables,result

def ordering(variables,result,spec):
 def value(term,rule):
  if term is None:return(0,None)
  kind,lex,datatype,lang=term
  if rule['kind']=='double_cast':assert kind=='literal';x=float(lex);assert math.isfinite(x)
  elif rule['kind']=='numeric':assert kind=='literal' and datatype and datatype.startswith(XSD);x=Decimal(lex);assert x.is_finite()
  elif rule['kind']=='iso_datetime':
   assert kind=='literal' and datatype==XSD+'dateTime' and lang is None
   assert re.fullmatch(r'[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}',lex)
   x=datetime.datetime.fromisoformat(lex)
  else:assert rule['kind']=='lexical' and kind==rule['term_kind'] and datatype==rule.get('datatype') and lang is None;x=lex
  return(1,x)
 previous=None
 for row in result:
  current=[value(row[variables.index(rule['var'])],rule) for rule in spec]
  if previous is not None:
   for a,b,rule in zip(previous,current,spec):
    if a==b:continue
    assert (a>b)==bool(rule.get('descending',False));break
  previous=current

def audit(packet):
 checksums=packet/'FILE_SHA256.json'
 if checksums.exists():
  for name,record in json.loads(checksums.read_text()).items():
   path=packet/name;assert path.is_file() and path.stat().st_size==record['bytes'] and sha(path)==record['sha256'],name
 stage=packet/'measurement';freeze_path=stage/'frozen/MEASUREMENT-FREEZE.json';freeze=json.loads(freeze_path.read_text());state=json.loads((stage/'supervisor.json').read_text());schedule=freeze['schedule']
 assert sha(freeze_path)==state['registration_sha256'];assert state['complete'] is True and state['eligible'] is True
 assert len(schedule)==120 and len({r['id'] for r in schedule})==120
 assert [e['id'] for e in state['events']]==[r['id'] for r in schedule]
 assert all(e['exit']==0 and e['reason'] is None for e in state['events'])
 for name,digest in freeze['source_sha256'].items():assert sha(stage/'frozen/source'/name)==digest
 for name,digest in freeze['driver_sha256'].items():assert sha(stage/'frozen'/name)==digest
 for variant,digest in freeze['translations'].items():assert sha(stage/'frozen'/(variant+'-translations-v4.json'))==digest
 for phase in ['preflight','postflight']:
  receipt=json.loads((stage/(phase+'.json')).read_text());assert receipt['pass'] is True and receipt['database_identities']==freeze['databases']
 refs=packet/'references';assert sha(refs/'provenance.json')==freeze['reference_provenance_sha256'];provenance=json.loads((refs/'provenance.json').read_text())
 records=[];bag_checks=order_checks=0
 for rule in schedule:
  ident=rule['id'];record=json.loads((stage/(ident+'.record.json')).read_text());gate=json.loads((stage/(ident+'.gate.json')).read_text())
  assert record['executed'] is True and gate['eligible'] is True and gate['pass_bag'] is True and gate['pass_order'] is True
  assert all(record[k]==rule[k]==gate[k] for k in ['id','pass','query','mapping'])
  raw=gzip.decompress((stage/(ident+'.json.gz')).read_bytes());assert hashlib.sha256(raw).hexdigest()==record['payload_sha256'];assert len(raw)==record['payload_bytes']
  refpath=refs/(rule['query']+'.json.gz');assert sha(refpath)==freeze['reference_gzip_sha256'][rule['query']]
  reference=gzip.decompress(refpath.read_bytes());assert hashlib.sha256(reference).hexdigest()==freeze['reference_json_sha256'][rule['query']]
  v,r=rows(json.loads(raw));rv,rr=rows(json.loads(reference));assert v==rv and collections.Counter(r)==collections.Counter(rr);bag_checks+=1
  spec=provenance['queries'][rule['query']].get('order')
  if spec:ordering(v,r,spec['keys']);ordering(rv,rr,spec['keys']);order_checks+=1
  fields=['open_seconds','execute_api_seconds','fetch_arrow_seconds','encode_json_seconds','query_to_full_json_seconds','close_seconds','open_through_close_seconds','peak_rss_bytes','peak_rss_at_full_json_bytes']
  assert all(type(record[k]) in [int,float] and math.isfinite(record[k]) and record[k]>=0 for k in fields)
  assert record['query_to_full_json_seconds']+1e-6>=sum(record[k] for k in ['execute_api_seconds','fetch_arrow_seconds','encode_json_seconds'])
  assert record['open_through_close_seconds']+1e-6>=sum(record[k] for k in ['open_seconds','query_to_full_json_seconds','close_seconds'])
  assert record['peak_rss_bytes']<=16*2**30
  records.append(record)
 assert bag_checks==120 and order_checks==54
 # All reference/gate work happened after the final query/export record was saved.
 assert min(p.stat().st_mtime for p in stage.glob('*.gate.json'))>=max(p.stat().st_mtime for p in stage.glob('*.record.json'))
 totals={}
 for phase in ['p1','p2']:
  totals[phase]={}
  for variant in ['generic','hybrid']:
   subset=[r for r in records if r['pass']==phase and r['mapping']==variant];assert len(subset)==20
   total=math.fsum(r['query_to_full_json_seconds'] for r in subset);assert math.isclose(total,state['totals'][phase][variant]['query_to_full_json_seconds'],rel_tol=1e-12,abs_tol=1e-10)
   totals[phase][variant]=total
 for event in state['events']:
  assert max(event['allocated_before'],event['allocated_after'])<46*2**30
  assert all(s['free']>48*2**30 and s['rss']<=16*2**30 for s in event['samples'])
 return {'pass':True,'registration_sha256':sha(freeze_path),'bags':bag_checks,'order_checks':order_checks,'requests':len(records),'primary_seconds':totals,'offline_only':True}
if __name__=='__main__':print(json.dumps(audit(Path(sys.argv[1])),indent=2))
