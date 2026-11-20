#!/usr/bin/env python3
"""Independent saved-evidence audit; never execute producer code or an engine."""
import json,hashlib,math,statistics,sys
from pathlib import Path
R=Path(sys.argv[1]).resolve()
def read(n):return json.loads((R/n).read_text())
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def eq(a,b):assert type(a) is type(b) and a==b,(a,b)
def positive(x):assert type(x) in (int,float) and math.isfinite(x) and x>0
manifest=read('PACKET_FILES.json');assert len(manifest)==237
assert {str(p.relative_to(R)) for p in R.rglob('*') if p.is_file()}==set(manifest)|{'PACKET_FILES.json'}
for n,h in manifest.items():eq(sha(R/n),h)
reg=read('scored-v2/registration.json');eq(sha(R/'scored-v2/registration.json'),'28fab704840430ca5651b9da78944ce4cb410b0f5285b6527acc773c15cf0d02')
eq(sha(R/'QUAL.SHA256'),reg['qualification_sha256'])
retained=[];omitted=[]
for line in (R/'QUAL.SHA256').read_text().splitlines():
 h,n=line.split('  ',1)
 if (R/n).exists():eq(sha(R/n),h);retained.append(n)
 else:omitted.append(n)
for name,key in [('score_a34.py','supervisor_sha256'),('probe.rs','probe_source_sha256'),('build.sh','build_script_sha256'),('launch.sh','launcher_sha256'),('launch_controls.sh','launch_controls_sha256'),('launch_controls.txt','launch_controls_output_sha256')]:eq(sha(R/name),reg['identities'][key])
rr=read('scored-v2/raft_receipt.json');eq(rr['registration_sha256'],sha(R/'scored-v2/registration.json'));eq(sha(R/'scored-v2/raft_message.txt'),rr['read_sha256']);assert rr['bound_unix']<read('scored-v2/attempt/started.json')['unix']
status=read('scored-v2/attempt/status.json');eq(set(status),{p['id'] for p in reg['phases']});assert len(status)==15
columns={'lubm1000-q9':3,'lubm1000-q14':1,'bsbm100-q010':2};widths={'lubm1000-q9':44190,'lubm1000-q14':7924765,'bsbm100-q010':5}
mems=set();observations=0;orders=0;cells={};clones=[]
def sums(s,w,ordered):
 for f in ['rows','cells','batches','checksum']:eq(s[f],w[f])
 assert s['ordered'] is ordered
 positive(s['rows']);eq(s['cells'],s['rows']*columns[cell])
for index,ph in enumerate(reg['phases']):
 cell=ph['cell'];eq(ph['rep'],index%5+1);eq(ph['order'],'arrow-first' if ph['rep']%2 else 'view-first')
 d='scored-v2/attempt/'+ph['id'];p=read(d+'/out/probe.json');rc=read(d+'/receipt.json');state=status[ph['id']];eq(state['status'],'pass');eq(state['probe'],p);eq(rc['result']['probe'],p)
 eq(rc['exit_code'],0);assert rc['oom_killed'] is False and rc['capped'] is False;assert rc['seconds']<reg['cap_s'];eq(rc['container_cleanup'],'removed');eq(rc['dropcache_helper_cleanup'],'removed');assert rc['private_clone_removed'] is True
 inv=rc['inventory'];assert inv['base_tree_unchanged'] is True and inv['sidecar_unchanged'] is True;assert set(inv['new_in_clone'])<= {'LOCK','wal.log'};eq(set(inv['out']),{'meminfo.start','memory.events.after-close','memory.max','memory.peak.after-close','probe.json','probe.stderr'})
 for field in ['oom','oom_kill','oom_group_kill']:eq(int(rc['memory_events'][field]),0)
 eq(int((R/d/'out/memory.max').read_text()),reg['container']['memory']);eq(int((R/d/'out/memory.peak.after-close').read_text()),rc['memory_peak_bytes']);eq(int((R/d/'out/meminfo.start').read_text().split()[1]),rc['memtotal_kB']);mems.add(rc['memtotal_kB'])
 ins=read(d+'/inspect.json');assert all(v is True for v in ins['checks'].values());eq(ins['mounts']['/bin/probe']['rw'],False);eq(ins['mounts']['/q.rq']['rw'],False);eq(ins['mounts']['/db']['rw'],True);clone=Path(ins['mounts']['/db']['source']);assert not clone.exists();clones.append(str(clone))
 assert ins['cmd'][-2:]==['phase',ph['order']] if cell!='bsbm100-q010' else ins['cmd'][-4:]==['phase',ph['order'],'--order-col','1']
 ordered=True if cell=='bsbm100-q010' else None;want=reg['qualification_values'][cell];eq(p['batch_rows'],8192);eq(p['order'],ph['order']);positive(p['stats_ready_ms'])
 for k in ['arrow','view']:sums(p['warmup'][k],want[k],ordered);observations+=1
 eq([t['path'] for t in p['timed']],['arrow','view'] if ph['order']=='arrow-first' else ['view','arrow'])
 for pos,t in enumerate(p['timed'],1):eq(t['position'],pos);positive(t['ms']);sums(t['sum'],want[t['path']],ordered);observations+=1
 it=p['instrumented'];positive(it['ids_ms']);positive(it['convert_ms']);sums(it['sum'],want['instr'],ordered);observations+=1
 for k in ['arrow','view','instr']:
  c=p['canonical'][k];sums(c['sum'],want[k],ordered);eq(c['bag_sha1'],reg['identities']['references'][cell]['bag_sha1']);observations+=1
  if ordered:
   rows=c['rows_in_order'];assert len(rows)==5;keys=[row.split('\t')[1].split('|')[1] for row in rows];assert keys==sorted(keys);orders+=1
 row=cells.setdefault(cell,{'arrow':[],'view':[],'ids':[],'convert':[],'positions':{'arrow':[],'view':[]}})
 for t in p['timed']:row[t['path']].append(t['ms']);row['positions'][t['path']].append(t['position'])
 row['ids'].append(it['ids_ms']);row['convert'].append(it['convert_ms'])
post=read('scored-v2/attempt/postcheck.json');assert post['identities_unchanged'] is True and post['qualification_unchanged'] is True and post['stopped_early'] is False;eq(mems,{post['cohort_MemTotal_kB']})
computed=[]
for cell,v in cells.items():
 row={'cell':cell,'rows':widths[cell]}
 for k in ['arrow','view','ids','convert']:
  x=v[k];assert len(x)==5;row[k]={'median_ms':statistics.median(x),'min_ms':min(x),'max_ms':max(x),'obs_ms':x}
 for k in ['convert','arrow','view']:row[k+'_us_per_row_median']=1000*row[k]['median_ms']/row['rows']
 a,b=v['arrow'],v['view'];row['arrow_vs_view_ranges']='arrow below view' if max(a)<min(b) else 'arrow above view' if min(a)>max(b) else 'ranges overlap';row['positions']=v['positions'];computed.append(row)
eq(computed,read('SUMMARY.json'))
assert (R/'scored-v2.run.log').read_text().rstrip().endswith('released')
print(json.dumps({'packet_files_verified':len(manifest),'qualification_files_rehashed':len(retained),'qualification_omitted_files':omitted,'phases':15,'observations_checked':observations,'canonical_q010_order_checks':orders,'summary_matches':True,'MemTotal_kB':list(mems),'clone_paths_absent':len(clones),'base_store_tree_independently_rehashed':False,'live_container_inventory_checked':False,'engine_executed':False,'packet_diff_is_formatted_summary':True,'exact_raw_diff_missing_from_v1':True,'summary':computed},indent=2))
