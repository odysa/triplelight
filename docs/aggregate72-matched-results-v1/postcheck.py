"""Independent replay of all registered requests and identities after the run."""
import collections,csv,hashlib,json,math,subprocess,sys,time
from pathlib import Path
stage=Path(__file__).resolve().parent;root=stage.parent
assert Path('/tmp/rdf-bench.lock/owner').read_text().strip()=='Jianlin task25 aggregate72 final result verification'
protocol_raw=(stage/'protocol.json').read_bytes();assert hashlib.sha256(protocol_raw).hexdigest()=='394b422c61409e27547ea471293e4406d9d8513f4bee6751762e4dc5d6e937cf'
protocol=json.loads(protocol_raw);out=stage/'round1';state=json.loads((out/'status.json').read_text());supervisor=json.loads((stage/'run-supervisor.json').read_text());launcher=json.loads((stage/'launcher-status.json').read_text())
assert state['complete'] and supervisor['complete'] and launcher['supervisor_exit']==0 and not launcher['owned_containers_remaining']
sys.path.insert(0,str(stage/'bench-source'));from aggregate_count_policy import compare
prep=root/'aggregate72-preparation-v1';reference=root/'aggregate72-reference-v2'
rules=json.loads((prep/'rules-freeze.json').read_text());binding=json.loads((reference/'reference-binding.json').read_text())
def sha(file):
 h=hashlib.sha256()
 with Path(file).open('rb') as stream:
  while b:=stream.read(8*1024*1024):h.update(b)
 return h.hexdigest()
def identities(folder):return {str(p.relative_to(folder)):{'bytes':p.stat().st_size,'sha256':sha(p)} for p in sorted(folder.rglob('*')) if p.is_file()}
assert sha(prep/'rules-freeze.json')==protocol['rules_sha256'];assert sha(reference/'reference-binding.json')==protocol['reference_binding_sha256']
for name,digest in protocol['source_files'].items():assert sha(stage/name)==digest
for name,digest in protocol['duck_module_files'].items():assert sha(Path(protocol['duck_modules'])/name)==digest
assert sha(stage/'target-linux-9a2f49b/release/rdfdb')==protocol['rdfdb_binary_sha256']
for dataset,config in protocol['datasets'].items():
 assert sha(config['source_path'])==config['source_sha256']
 assert identities(Path(config['rdfdb_store']))==config['rdfdb_files']
 assert identities(Path(config['qlever_store']))==config['qlever_files']
 assert sha(config['duckdb_store'])==config['duckdb_sha256']
records=[];phases={};expected_phases=[];reference_cache={}
for pass_number,systems in enumerate(protocol['engine_orders'],1):
 for dataset in protocol['dataset_orders'][pass_number-1]:
  requested={name:rec for name,rec in rules['queries'].items() if rec['dataset']==dataset}
  for system in systems:
   key=f'pass{pass_number}-{dataset}-{system}';expected_phases.append(key);phase=out/key;entry=state['phases'][key];assert entry['complete'] and entry['immutable_store_check_pass']
   assert set(entry['requests'])=={part+'/'+name for part in ['warmup','measured'] for name in requested}
   assert entry['graph_count_before']==entry['graph_count_after']==protocol['datasets'][dataset]['graph_count']
   resources=json.loads((phase/'actual-resources.json').read_text());assert resources=={'cpu_nanos':4000000000,'memory':12*1024**3,'memory_swap_total':12*1024**3,'image_id':protocol['image_ids'][system]}
   ended=json.loads((phase/'container-after-stop.json').read_text());assert not ended['Running'] and not ended['OOMKilled']
   memory=json.loads((phase/'cgroup-memory.json').read_text());assert memory['exit_code']==0;peak,limit,swap=map(int,memory['stdout'].split());assert limit==12*1024**3 and swap==0 and peak>=0
   before=json.loads((phase/'store-before.json').read_text());after=json.loads((phase/'store-after.json').read_text());assert before==after
   if system=='rdfdb':assert before==protocol['datasets'][dataset]['rdfdb_files']
   if system=='qlever':assert before==protocol['datasets'][dataset]['qlever_files']
   if system=='duckdb':assert before==protocol['datasets'][dataset]['duckdb_sha256']
   measured=[];by_hash=collections.defaultdict(list);all_results=[]
   for part in ['warmup','measured']:
    for name,rec in requested.items():
     result=entry['requests'][part+'/'+name];stem=Path(name).stem;body=phase/part/(stem+'.json')
     assert sha(prep/'workload'/name)==rec['query_sha256']==result['query_sha256'];assert sha(body)==result['body_sha256'];assert body.stat().st_size==result['body_bytes']
     if name not in reference_cache:
      ref=reference/dataset/(stem+'.json');assert sha(ref)==binding['references'][name]['sha256'];reference_cache[name]=json.loads(ref.read_text())
     actual=json.loads(body.read_text());replayed=compare(actual,reference_cache[name],rec['columns'])
     assert all(replayed[field]==result[field] for field in ['primary_pass','count_compatible_pass','rows']);assert replayed['count_compatible_pass']
     latency=result['elapsed_seconds'];assert math.isfinite(latency) and latency>0
     if system=='qlever':
      clear=json.loads((phase/part/(stem+'-clear.json')).read_text());assert len(clear)==5 and all(value==0 for value in clear.values())
     record={'phase':key,'pass':pass_number,'dataset':dataset,'system':system,'part':part,'query':name,'query_sha256':rec['query_sha256'],'elapsed_seconds':latency,'primary_pass':replayed['primary_pass'],'count_compatible_pass':True,'rows':replayed['rows'],'body_bytes':result['body_bytes'],'body_sha256':result['body_sha256']};records.append(record);all_results.append(record)
     if part=='measured':measured.append(record);by_hash[rec['query_sha256']].append(latency)
   unique=[math.fsum(group)/len(group) for group in by_hash.values()]
   phases[key]={'pass':pass_number,'dataset':dataset,'system':system,'scored_queries':len(measured),'warmup_queries':len(requested),'primary_scored_pass':sum(r['primary_pass'] for r in measured),'primary_all_pass':sum(r['primary_pass'] for r in all_results),'count_compatible_all_pass':len(all_results),'labeled_total_seconds':math.fsum(r['elapsed_seconds'] for r in measured),'labeled_geomean_seconds':math.exp(math.fsum(math.log(r['elapsed_seconds']) for r in measured)/len(measured)),'unique_query_hashes':len(unique),'unique_hash_mean_sum_seconds':math.fsum(unique),'unique_hash_mean_geomean_seconds':math.exp(math.fsum(math.log(x) for x in unique)/len(unique)),'container_cgroup_peak_bytes':peak,'container_shutdown_exit':ended['ExitCode']}
assert set(state['phases'])==set(expected_phases) and len(phases)==16
assert len(records)==1152 and sum(r['part']=='measured' for r in records)==576
combined={}
for pass_number in [1,2]:
 for system in protocol['engine_orders'][0]:
  selected=[x for x in phases.values() if x['pass']==pass_number and x['system']==system]
  measured=[r for r in records if r['pass']==pass_number and r['system']==system and r['part']=='measured']
  combined[f'pass{pass_number}-{system}']={'pass':pass_number,'system':system,'scored_queries':len(measured),'primary_scored_pass':sum(r['primary_pass'] for r in measured),'count_compatible_scored_pass':len(measured),'labeled_total_seconds':math.fsum(x['labeled_total_seconds'] for x in selected),'unique_hash_mean_sum_seconds':math.fsum(x['unique_hash_mean_sum_seconds'] for x in selected),'labeled_geomean_seconds':math.exp(math.fsum(math.log(r['elapsed_seconds']) for r in measured)/len(measured)),'max_phase_cgroup_peak_bytes':max(x['container_cgroup_peak_bytes'] for x in selected)}
probe=subprocess.run(['docker','ps','-a','--filter','name=rdf-jianlin-agg72-match-','--format','{{.Names}}'],capture_output=True,text=True,check=True);assert not probe.stdout.strip()
pids=[json.loads((stage/'runner-process.json').read_text())['pid'],supervisor['pid']];ps=subprocess.run(['ps','-p',','.join(map(str,pids)),'-o','pid,comm'],capture_output=True,text=True);assert len(ps.stdout.strip().splitlines())<=1
samples=[json.loads(line) for line in (stage/'run-supervisor.samples.jsonl').read_text().splitlines()]
summary={'protocol_sha256':hashlib.sha256(protocol_raw).hexdigest(),'complete':True,'requests':len(records),'warmup_requests':576,'scored_requests':576,'primary_all_pass':sum(r['primary_pass'] for r in records),'count_compatible_all_pass':len(records),'qlever_cache_acknowledgements_verified':sum(r['system']=='qlever' for r in records),'source_reference_module_binary_store_checks_pass':True,'owned_containers_removed':True,'recorded_pids_absent':pids,'ps_stdout':ps.stdout,'sampled_max_root_allocated_bytes':max(x['allocated_bytes'] for x in samples),'sampled_min_free_bytes':min(x['free_bytes'] for x in samples),'phases':phases,'combined':combined,'verified_unix':time.time()}
(out/'verified-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
with (out/'query-records.csv').open('w') as stream:
 writer=csv.DictWriter(stream,fieldnames=list(records[0]));writer.writeheader();writer.writerows(records)
with (out/'phase-summary.csv').open('w') as stream:
 writer=csv.DictWriter(stream,fieldnames=['phase',*next(iter(phases.values()))]);writer.writeheader();writer.writerows({'phase':name,**entry} for name,entry in phases.items())
print('Verified',len(records),'COUNT-compatible results, 16 phases; primary',summary['primary_all_pass'])
print(json.dumps(combined,indent=2))
