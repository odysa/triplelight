import json,hashlib,runpy,statistics,math
from pathlib import Path
S=Path(__file__).resolve().parent
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
packet=json.loads((S/'PACKET_FILES.json').read_text())
for n,h in packet.items():assert sha(S/n)==h,n
assert len(packet)==294
review=Path('scratch/a36-registration-review-v3')
assert sha(S/'score_a36.py')==sha(review/'score_a36.py')
m=runpy.run_path(str(review/'score_a36.py'))
reg=json.loads((S/'scored-v3/registration.json').read_text());assert sha(S/'scored-v3/registration.json')=='9616fd024e3b1c92a9649247476b61e0158404cb3088f25f8da65c7fbff797d6'
assert reg['phases']==m['schedule']()
status=json.loads((S/'scored-v3/attempt/status.json').read_text());assert list(status)==[p['id'] for p in reg['phases']]
post=json.loads((S/'scored-v3/attempt/postcheck.json').read_text());assert post['identities_unchanged'] is True and post['qualification_unchanged'] is True and post['stopped_early'] is False
summary=json.loads((S/'SUMMARY.json').read_text());assert len(summary)==40
rows=[];windows=[];ov=(1795174616,1795174709);names=['hits','misses','reads','bytes','evictions']
for ph in reg['phases']:
 d=S/'scored-v3/attempt'/ph['id'];rc=json.loads((d/'receipt.json').read_text());raw=json.loads((d/'out/probe.json').read_text());ins=json.loads((d/'inspect.json').read_text())
 assert status[ph['id']]['status']=='pass' and raw==status[ph['id']]['probe']==rc['result']['probe']
 assert not m['phase_gate'](ph,raw,reg)
 assert rc['exit_code']==0 and rc['oom_killed'] is False and rc['capped'] is False
 assert rc['dropcache_helper_cleanup']=='removed' and rc['container_cleanup']=='removed' and rc['private_clone_removed'] is True
 assert all(rc['inventory'][k] is True for k in ['base_tree_unchanged','sidecar_unchanged'])
 assert rc['inventory']['out']==sorted(m['ALLOWED_OUT']) and set(rc['inventory']['new_in_clone'])<=m['ALLOWED_NEW_DB']
 assert all(ins['checks'].values()) and ins['mounts']['/db']['rw'] is True and ins['mounts']['/bin/probe']['rw'] is False and ins['mounts']['/q.rq']['rw'] is False
 expected_env='TRIPLELIGHT_TRACE_BLOCK_CACHE=1'
 assert (expected_env in ins['env']) is ph['trace']
 assert all(not e.startswith('TRIPLELIGHT_TRACE_BLOCK_CACHE=') for e in ins['env']) if not ph['trace'] else True
 assert ins['cmd'][4:]==['/db','/q.rq','phase',ph['order']]+(['--locality'] if ph['cell'].endswith('-q9') else [])
 events=dict(l.split() for l in (d/'out/memory.events.after-close').read_text().splitlines());assert events==rc['memory_events'] and all(int(events[k])==0 for k in ['oom','oom_kill'])
 assert int((d/'out/memory.max').read_text())==12<<30
 assert rc['memtotal_kB']==post['cohort_MemTotal_kB']==16414236
 assert (d/'container.log').stat().st_size==0
 # Archive preserves log mtime. Since the log stayed empty, it records its opening next to t0.
 # This is a reconstructed host timestamp proxy, not an exact recorded container start.
 start=(d/'container.log').stat().st_mtime;end=start+rc['seconds'];overlap=start<=ov[1] and end>=ov[0]
 windows.append({'phase':ph['id'],'empty_log_mtime_start_proxy':start,'end_proxy':end,'overlap':overlap})
 for x in raw['passes']:
  delta=lambda w:[[w['after'][i][j]-w['before'][i][j] for j in range(5)] for i in range(8)]
  q=delta(x['query_window']);c=delta(x['convert_window']);r={'phase':ph['id'],'cell':ph['cell'],'trace':ph['trace'],'pass':x['pass'],'position':x['position'],'overlap':overlap,'total_ms':x['total_ms'],'ids_ms':x['ids_ms'],'decode_ms':(x['stages'] or {}).get('decode_ms'),'query_delta':q,'convert_delta':c,'slot7_convert':dict(zip(names,c[7])),'nonzero_slots_convert':[i for i in range(8) if any(c[i])],'nonzero_slots_query':[i for i in range(8) if any(q[i])]}
  recorded=next(y for y in summary if y['phase']==r['phase'] and y['pass']==r['pass'])
  for k,v in r.items():assert recorded[k]==v,(ph['id'],k)
  if not ph['trace']:assert not any(v for mat in [q,c] for rr in mat for v in rr)
  rows.append(r)
allstats={}
for cell in ['lubm1000-q9','lubm1000-q14']:
 on=[r for r in rows if r['cell']==cell and r['trace']];assert len(on)==10
 stats={k:{'median':statistics.median([r['slot7_convert'][k] for r in on]),'min':min(r['slot7_convert'][k] for r in on),'max':max(r['slot7_convert'][k] for r in on)} for k in names}
 for v in stats.values():assert v['min']==v['max']
 hits=stats['hits']['median'];miss=stats['misses']['median'];occ=121633 if cell.endswith('-q9') else 7924765
 stats['miss_fraction']=miss/(hits+miss);stats['batch_distinct_id_occurrences_per_miss_event']=occ/miss
 stats['timing_ms']={str(tr):{'median':statistics.median([r['total_ms'] for r in rows if r['cell']==cell and r['trace'] is tr]),'min':min(r['total_ms'] for r in rows if r['cell']==cell and r['trace'] is tr),'max':max(r['total_ms'] for r in rows if r['cell']==cell and r['trace'] is tr)} for tr in [True,False]}
 allstats[cell]=stats
v2=json.loads((S/'scored-v2/attempt/status.json').read_text());assert sum(v['status']=='not-run' for v in v2.values())==19 and list(v2.values())[0]['status']=='gate-failed'
assert not (S/'scored-v1-unrun/attempt').exists()
report={'packet_sha256':'15af092f0cc98da4ce15f5c4f4914a03c26ff132bb670898d02b5b10d824e55e','manifest_files':len(packet),'phases':20,'pass_summaries':len(rows),'all_checks_pass':True,'stats':allstats,'overlap_phases':[w['phase'] for w in windows if w['overlap']],'windows':windows,'summary_limitation':'Original start_unix uses phase directory birth time, while seconds begins at Docker attach after preparation. That hybrid is not an exact interval and directory birth time is not archived. Empty log mtime provides a portable reconstructed start proxy and yields the same overlap flags. Preserve this provenance and correct the summary before publication.','earlier_attempts':'v1 unrun; v2 phase1 gate-failed,19not-run; no pooling','scope':'No engine, Docker or new large-store rehash. Raw probes and phase gates, receipts, saved resource/inspection checks and derived summaries replayed.'}
(S/'INDEPENDENT_AUDIT.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({k:v for k,v in report.items() if k not in ['windows','stats']},indent=2));print(json.dumps(allstats,indent=2))
