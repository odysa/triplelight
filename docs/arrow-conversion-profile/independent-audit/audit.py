#!/usr/bin/env python3
"""Independent saved-evidence checks, no engine or archived-code execution."""
import json,hashlib,math,statistics,sys
from pathlib import Path
R=Path(sys.argv[1]).resolve()
def read(n):return json.loads((R/n).read_text())
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def eq(a,b):assert type(a) is type(b) and a==b,(a,b)
def num(x,positive=False):assert type(x) in (int,float) and math.isfinite(x) and (x>0 if positive else x>=0),x
STAGES=['collect_ms','decode_ms','scratch_ms','build_ms','consume_ms','release_ms']
manifest=read('PACKET_FILES.json');eq(len(manifest),167);eq(set(manifest)|{'PACKET_FILES.json'},{str(p.relative_to(R)) for p in R.rglob('*') if p.is_file()})
for n,h in manifest.items():eq(sha(R/n),h)
reg=read('scored-v2/registration.json');eq(sha(R/'scored-v2/registration.json'),'bc6ebc65a789927ca74f69d98fcaf4cea1054c10a040bead9a9772be7c4b6e4a');eq(sha(R/'QUAL.SHA256'),reg['qualification_sha256'])
retained=[];omitted=[]
for line in (R/'QUAL.SHA256').read_text().splitlines():
 h,n=line.split('  ',1)
 if (R/n).exists():eq(sha(R/n),h);retained.append(n)
 else:omitted.append(n)
for n,key in [('score_a35.py','supervisor_sha256'),('probe.rs','probe_source_sha256'),('build.sh','build_script_sha256'),('launch.sh','launcher_sha256'),('launch_controls.sh','launch_controls_sha256'),('launch_controls.txt','launch_controls_output_sha256'),('probe-worktree.raw.diff','worktree_diff_sha256'),('batch-source-comparison.diff','batch_source_comparison_sha256')]:eq(sha(R/n),reg['identities'][key])
rr=read('scored-v2/raft_receipt.json');eq(rr['registration_sha256'],sha(R/'scored-v2/registration.json'));eq(sha(R/'scored-v2/raft_message.txt'),rr['read_sha256']);assert rr['bound_unix']<read('scored-v2/attempt/started.json')['unix']
status=read('scored-v2/attempt/status.json');eq(set(status),{p['id'] for p in reg['phases']});eq(len(status),10)
mems=set();obs_checked=0;loc_checks=0;computed={};supplement={};widths={'lubm1000-q9':3,'lubm1000-q14':1}
def sums(v,w):
 for k in ['rows','cells','batches','checksum']:eq(v[k],w[k])
for index,ph in enumerate(reg['phases']):
 cell=ph['cell'];eq(ph['rep'],index%5+1);eq(ph['order'],'control-first' if ph['rep']%2 else 'profiled-first');d='scored-v2/attempt/'+ph['id'];p=read(d+'/out/probe.json');rc=read(d+'/receipt.json');st=status[ph['id']];eq(st['status'],'pass');eq(st['probe'],p);eq(rc['result']['probe'],p)
 eq(rc['exit_code'],0);assert rc['oom_killed'] is False and rc['capped'] is False;assert rc['seconds']<reg['cap_s'];eq(rc['container_cleanup'],'removed');eq(rc['dropcache_helper_cleanup'],'removed');assert rc['private_clone_removed'] is True
 inv=rc['inventory'];assert inv['base_tree_unchanged'] is True and inv['sidecar_unchanged'] is True;assert set(inv['new_in_clone'])<={'LOCK','wal.log'}
 for k in ['oom','oom_kill','oom_group_kill']:eq(int(rc['memory_events'][k]),0)
 eq(int((R/d/'out/memory.max').read_text()),reg['container']['memory']);eq(int((R/d/'out/memory.peak.after-close').read_text()),rc['memory_peak_bytes']);eq(int((R/d/'out/meminfo.start').read_text().split()[1]),rc['memtotal_kB']);mems.add(rc['memtotal_kB'])
 ins=read(d+'/inspect.json');assert all(v is True for v in ins['checks'].values());assert not Path(ins['mounts']['/db']['source']).exists();assert ins['mounts']['/bin/probe']['rw'] is False and ins['mounts']['/q.rq']['rw'] is False;eq(ins['cmd'][-3:] if cell.endswith('q9') else ins['cmd'][-2:],['phase',ph['order'],'--locality'] if cell.endswith('q9') else ['phase',ph['order']])
 want=reg['frozen_values'][cell];eq(p['mode'],'phase');eq(p['order'],ph['order']);eq(p['batch_rows'],8192);num(p['stats_ready_ms'],True);sums(p['warmup'],want);obs_checked+=1
 eq([x['pass'] for x in p['passes']],['control','profiled'] if ph['order']=='control-first' else ['profiled','control'])
 for pos,x in enumerate(p['passes'],1):
  eq(x['position'],pos);num(x['ids_ms'],True);num(x['total_ms'],True);sums(x['sum'],want);obs_checked+=1
  if x['pass']=='control':assert x['stages'] is None and 'stage_sum_ms' not in x and 'remainder_ms' not in x
  else:
   for k in STAGES:num(x['stages'][k])
   eq(type(x['stages']['distinct_ids_decoded']),int);assert x['stages']['distinct_ids_decoded']>=0
   total=sum(x['stages'][k] for k in STAGES);assert abs(total-x['stage_sum_ms'])<=.001;assert abs(x['total_ms']-total-x['remainder_ms'])<=.002
   assert type(x['remainder_ms']) in (int,float) and math.isfinite(x['remainder_ms'])
 ps={x['pass']:x for x in p['passes']};co=ps['control'];pr=ps['profiled'];c=computed.setdefault(cell,{'obs':[]})
 row={'rep':ph['rep'],'order':ph['order'],'control_total_ms':co['total_ms'],'control_pos':co['position'],'profiled_total_ms':pr['total_ms'],'profiled_pos':pr['position'],'control_ids_ms':co['ids_ms'],'profiled_ids_ms':pr['ids_ms'],**{k:pr['stages'][k] for k in STAGES},'stage_sum_ms':pr['stage_sum_ms'],'remainder_ms':pr['remainder_ms'],'distinct_ids_decoded':pr['stages']['distinct_ids_decoded'],'rows':pr['sum']['rows'],'cells':pr['sum']['cells']};c['obs'].append(row)
 if cell.endswith('q9'):
  loc=p['locality'];sums(loc['sum'],want);obs_checked+=1;pb=loc['per_batch_distinct_cells_gaps_runs'];eq(len(pb),want['batches']);eq(loc['batches'],want['batches'])
  for i,(distinct,cells,gaps,runs) in enumerate(pb):
   for val in [distinct,cells,gaps,runs]:eq(type(val),int);assert val>=0
   eq(cells,min(8192,want['rows']-i*8192)*widths[cell]);assert distinct<=cells;eq(gaps,max(distinct-1,0));assert (runs==0)==(distinct==0) and runs<=distinct
  gb,rb=loc['gap_buckets'],loc['run_buckets'];eq(len(gb),5);eq(len(rb),4)
  for x in gb+rb:eq(type(x),int);assert x>=0
  eq(sum(gb),sum(v[2] for v in pb));eq(gb[0],sum(v[0]-v[3] for v in pb));eq(sum(rb),sum(v[3] for v in pb));eq(loc['runs'],sum(rb));eq(loc['distinct_ids_sum'],sum(v[0] for v in pb));eq(loc['cells'],sum(v[1] for v in pb));eq(loc['cells'],want['cells']);eq(loc['distinct_ids_sum'],pr['stages']['distinct_ids_decoded'])
  assert rb[0]+2*rb[1]+16*rb[2]+256*rb[3]<=loc['distinct_ids_sum'];assert rb[3]>0 or rb[0]+15*rb[1]+255*rb[2]>=loc['distinct_ids_sum'];loc_checks+=1
  summary_loc={k:v for k,v in loc.items() if k!='per_batch_distinct_cells_gaps_runs'}
  if 'locality' in c:eq(c['locality'],summary_loc)
  c['locality']=summary_loc
 else:assert 'locality' not in p
post=read('scored-v2/attempt/postcheck.json');assert post['identities_unchanged'] is True and post['qualification_unchanged'] is True and post['stopped_early'] is False;eq(mems,{post['cohort_MemTotal_kB']});assert (R/'scored-v2.run.log').read_text().rstrip().endswith('released')
fields=['control_total_ms','profiled_total_ms','control_ids_ms','profiled_ids_ms',*STAGES,'stage_sum_ms','remainder_ms']
for cell,c in computed.items():
 rows=c['obs'];eq(len(rows),5);c['summary']={k:{'median':statistics.median(x[k] for x in rows),'min':min(x[k] for x in rows),'max':max(x[k] for x in rows)} for k in fields};c['share_of_profiled_total_median']={k:c['summary'][k]['median']/c['summary']['profiled_total_ms']['median'] for k in STAGES};c['decode_us_per_distinct_id_median']=statistics.median(1000*x['decode_ms']/x['distinct_ids_decoded'] for x in rows)
 paired=[x['profiled_total_ms']-x['control_total_ms'] for x in rows];ratio=[x['decode_ms']/x['profiled_total_ms'] for x in rows];position={}
 for path in ['profiled','control']:
  first=[x[path+'_total_ms'] for x in rows if x[path+'_pos']==1];second=[x[path+'_total_ms'] for x in rows if x[path+'_pos']==2];position[path]={'first_range_ms':[min(first),max(first)],'second_range_ms':[min(second),max(second)],'all_second_above_all_first':min(second)>max(first)}
 supplement[cell]={'paired_differences_ms':paired,'paired_difference_median_ms':statistics.median(paired),'difference_of_medians_ms':c['summary']['profiled_total_ms']['median']-c['summary']['control_total_ms']['median'],'median_per_observation_decode_share':statistics.median(ratio),'ratio_of_decode_and_profiled_medians':c['share_of_profiled_total_median']['decode_ms'],'position':position}
eq(computed,read('SUMMARY.json'))
print(json.dumps({'packet_files_verified':167,'qualification_files_rehashed':len(retained),'qualification_omitted_files':omitted,'phases':10,'observation_summaries_checked':obs_checked,'locality_observations_checked':loc_checks,'summary_matches':True,'base_store_tree_independently_rehashed':False,'live_container_inventory_checked':False,'engine_executed':False,'supplement':supplement,'summary':computed},indent=2))
