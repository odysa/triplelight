"""Independent A2 packet replay. Reads archived bytes without invoking the engine or author summary."""
import argparse
from collections import Counter, defaultdict
from hashlib import sha256
import json
import math
from pathlib import Path, PurePosixPath
from statistics import median
import tarfile

REGISTRATION = 'a40674ec9c233c114aff8c22d56fdb2f8ec85c3e2e4fe3ba6f471b9b061d74b1'
RECEIPT_MSG = '98b796e9-b0e6-498a-b721-67476bf05367'
BUDGETS = {'1x': 4007, '1.5x': 6011, '2x': 8014}
Q = {'lubm1000-q9': (44190, 132570, 6, '9627ff802f76b79f'),
     'lubm1000-q14': (7924765, 7924765, 968, '83d42706ddbea7a9')}
ORDER = ('control-first', 'profiled-first', 'control-first', 'profiled-first', 'control-first')
BUDGET_ORDER = (('1x','1.5x','2x'), ('1.5x','2x','1x'), ('2x','1x','1.5x'), ('1x','1.5x','2x'), ('1.5x','2x','1x'))
STAGES = ('collect_ms','decode_ms','scratch_ms','build_ms','consume_ms','release_ms')
COUNTERS = ('hits','misses','reads','bytes','evictions')
MEMORY = ('anon','file','file_mapped','active_file','inactive_file','pgfault','pgmajfault','pgscan','pgsteal','workingset_refault_file')
SYSTEM = {'memory_stat': MEMORY, 'proc_io': ('rchar','read_bytes','syscr')}
CUMULATIVE = {'memory_stat': MEMORY[5:], 'proc_io': SYSTEM['proc_io']}
ATTEMPT = 'scored-v2/attempt/'


def natural(value):
    return type(value) is int and value >= 0


def time_valid(value):
    return type(value) in (float, int) and math.isfinite(value) and value >= 0


def table(values):
    return [min(values), median(values), max(values)]


def packet_bytes(path):
    files = {}
    apple_double = {}
    with tarfile.open(path) as archive:
        for item in archive:
            name = item.name.removeprefix('./')
            assert not PurePosixPath(name).is_absolute() and '..' not in PurePosixPath(name).parts
            if item.isdir():
                continue
            assert item.isfile() and 0 <= item.size < 20 * 2**20
            content = archive.extractfile(item).read()
            if PurePosixPath(name).name.startswith('._'):
                assert content[:4] == bytes.fromhex('00051607')
                assert name not in apple_double
                apple_double[name] = sha256(content).hexdigest()
            else:
                assert name not in files
                files[name] = content
    manifest = json.loads(files.pop('PACKET_FILES.json'))
    assert len(manifest) == 288
    assert {k:sha256(v).hexdigest() for k,v in files.items()} == manifest
    assert all((PurePosixPath(name).name[2:]) for name in apple_double)
    return files, apple_double


def match_sum(raw, want):
    assert tuple(raw[k] for k in ('rows','cells','batches','checksum')) == want[:4]
    assert all(natural(raw[k]) for k in ('rows','cells','batches'))


def locality(record, want):
    match_sum(record['sum'], want)
    rows, cells, batch_count, _ = want
    pieces = record['per_batch_distinct_cells_gaps_runs']
    assert record['batches'] == len(pieces) == batch_count
    for i, (distinct, width, gaps, runs) in enumerate(pieces):
        assert all(map(natural, (distinct, width, gaps, runs)))
        assert width == min(8192, rows-i*8192)*(cells//rows)
        assert distinct <= width and gaps == max(0, distinct-1)
        assert (runs == 0) == (distinct == 0) and runs <= distinct
    distinct, count, gaps, runs = map(sum, zip(*pieces))
    assert (record['distinct_ids_sum'], record['cells'], record['runs']) == (distinct, count, runs)
    assert count == cells
    gb, rb = record['gap_buckets'], record['run_buckets']
    assert len(gb) == 5 and len(rb) == 4 and all(map(natural, gb+rb))
    assert sum(gb) == gaps and sum(rb) == runs and gb[0] == distinct-runs
    assert distinct >= sum(x*y for x,y in zip(rb,(1,2,16,256)))
    assert rb[3] or distinct <= sum(x*y for x,y in zip(rb,(1,15,255,0)))
    return distinct


def check_phase(phase, raw, receipt, inspection, status, files):
    phase_id = phase['id']
    path = ATTEMPT+phase_id+'/'
    assert receipt['phase'] == phase_id and receipt['container'] == 'a37-attempt-'+phase_id
    assert receipt['args'] == ['phase',phase['order'],'--budget-mb',str(phase['budget_mb']),
                               *(['--locality'] if phase['cell'].endswith('q9') else [])]
    assert receipt['exit_code'] == 0 and receipt['oom_killed'] is receipt['capped'] is False
    assert receipt['container_cleanup'] == receipt['dropcache_helper_cleanup'] == 'removed'
    assert receipt['private_clone_removed'] is True
    assert receipt['inventory']['base_tree_unchanged'] is receipt['inventory']['sidecar_unchanged'] is True
    assert set(receipt['inventory']['new_in_clone']) <= {'LOCK','wal.log'}
    assert receipt['result']['status'] == 'ran' and receipt['result']['probe'] == status['probe'] == raw
    assert status['status'] == 'pass' and not status['problems']
    assert not files[path+'out/probe.stderr'] and not receipt['probe_stderr']
    expected_inspection = {'image_digest','platform','cpus','memory','network','cgroupns',
                           'read_only_root','env','mounts','trace_env'}
    assert set(inspection['checks']) == expected_inspection and all(v is True for v in inspection['checks'].values())
    assert set(inspection['mounts']) == {'/db','/out','/q.rq','/bin/probe'}
    assert all(inspection['mounts'][dest]['rw'] is (dest in ('/db','/out')) for dest in inspection['mounts'])
    assert {'RAYON_NUM_THREADS=4','TRIPLELIGHT_TRACE_BLOCK_CACHE=1'} <= set(inspection['env'])
    assert inspection['cmd'][4:] == ['/db','/q.rq',*receipt['args']]
    events = dict(line.split() for line in files[path+'out/memory.events.after-close'].decode().splitlines())
    assert events == receipt['memory_events'] and events['oom'] == events['oom_kill'] == '0'
    peak = int(files[path+'out/memory.peak.after-close'])
    memtotal = int(files[path+'out/meminfo.start'].decode().split()[1])
    assert peak == receipt['memory_peak_bytes'] and 0 < peak < int(files[path+'out/memory.max']) == 12*2**30
    assert memtotal == receipt['memtotal_kB'] == 16414236
    assert raw['physical_memory_bytes'] == memtotal*1024
    assert raw['derived_default_budget_mb'] == 4007 == raw['physical_memory_bytes']//4//2**20
    assert raw['stats_origin']['kind'] == 'Persisted' and raw['mode'] == 'phase' and raw['batch_rows'] == 8192
    assert raw['trace_enabled'] is raw['trace_env_present'] is True
    assert raw['order'] == phase['order'] and raw['budget_mb_set'] == phase['budget_mb']
    budget = raw['budget_mb_set']*2**20
    assert raw['cache_capacities'] == {
        **{f'dictionary_by_{name}_shard_budget_bytes':budget//3//16 for name in ('id','term')},
        **{f'index{i}_shard_capacity_blocks':budget//(9*16*256*24) for i in range(6)},
    }
    match_sum(raw['warmup'], Q[phase['cell']])
    distinct = None
    if phase['cell'].endswith('q9'):
        distinct = locality(raw['locality'],Q[phase['cell']])
    else:
        assert 'locality' not in raw
    expected_passes = ('control','profiled') if phase['order']=='control-first' else ('profiled','control')
    assert tuple(x['pass'] for x in raw['passes']) == expected_passes
    prior_counters = None
    prior_system = None
    rows = []
    for position, obs in enumerate(raw['passes'],1):
        assert obs['position'] == position
        want = Q[phase['cell']]
        match_sum(obs['sum'],want)
        assert time_valid(obs['ids_ms']) and obs['ids_ms']>0 and time_valid(obs['total_ms']) and obs['total_ms']>0
        stages = obs['stages']
        if obs['pass'] == 'profiled':
            assert all(time_valid(stages[k]) for k in STAGES)
            assert natural(stages['distinct_ids_decoded'])
            if distinct is not None:
                assert stages['distinct_ids_decoded'] == distinct
            value = math.fsum(stages[k] for k in STAGES)
            assert abs(value-obs['stage_sum_ms']) <= .001
            assert abs(obs['total_ms']-value-obs['remainder_ms']) <= .002
        else:
            assert stages is None and 'stage_sum_ms' not in obs and 'remainder_ms' not in obs
        delta = {}
        sysdelta = {}
        for window in ('query_window','convert_window'):
            data = obs[window]
            for side in ('before','after'):
                matrix = data[side]
                assert len(matrix)==8 and all(len(r)==5 and all(map(natural,r)) for r in matrix)
                flat = tuple(value for row in matrix for value in row)
                if prior_counters is not None:
                    assert all(before<=after for before,after in zip(prior_counters,flat,strict=True))
                prior_counters = flat
                telemetry = data['sys_'+side]
                assert all(set(telemetry[group])==set(keys) and all(natural(v) for v in telemetry[group].values())
                           for group,keys in SYSTEM.items())
                if prior_system is not None:
                    assert all(prior_system[g][k] <= telemetry[g][k] for g,keys in CUMULATIVE.items() for k in keys)
                prior_system=telemetry
            delta[window]=[
                dict(zip(COUNTERS,(end-start for start,end in zip(before,after,strict=True)),strict=True))
                for before,after in zip(data['before'],data['after'],strict=True)]
            sysdelta[window]={g:{k:data['sys_after'][g][k]-data['sys_before'][g][k] for k in keys}
                              for g,keys in SYSTEM.items()}
        rows.append({'phase':phase_id,'cell':phase['cell'].split('-')[1], 'rep':phase['rep'],
                     'budget':phase['budget'],'budget_position':phase['budget_position'],
                     'pass':obs['pass'],'position':position,'ids_ms':obs['ids_ms'],
                     'conversion_total_ms':obs['total_ms'],'decode_ms':stages and stages['decode_ms'],
                     'slot7':delta['convert_window'][7], 'other_slots':[
                       i for i,row in enumerate(delta['convert_window'][:7]) if any(row.values())],
                     'query_slots':delta['query_window'], 'sysdelta':sysdelta,
                     'peak_bytes':peak,'phase_seconds':receipt['seconds']})
    return rows


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('packet',type=Path)
    args=parser.parse_args()
    files, metadata=packet_bytes(args.packet)
    read_json=lambda path: json.loads(files[path])
    registration=read_json('scored-v2/registration.json')
    receipt=read_json('scored-v2/raft_receipt.json')
    started=read_json(ATTEMPT+'started.json')
    post=read_json(ATTEMPT+'postcheck.json')
    assert sha256(files['scored-v2/registration.json']).hexdigest()==REGISTRATION
    assert receipt['registration_sha256']==started['registration_sha256']==REGISTRATION
    assert receipt['task']=='#sota-rdf-db:#34' and receipt['message_id']==started['raft_message_id']==RECEIPT_MSG
    msg=files['scored-v2/raft_message.txt']
    assert sha256(msg).hexdigest()==receipt['read_sha256'] and REGISTRATION.encode() in msg
    assert registration['registered_unix'] < receipt['bound_unix'] < started['unix'] < post['unix']
    assert post['identities_unchanged'] is post['qualification_unchanged'] is True and post['stopped_early'] is False
    for identity, name in (('supervisor_sha256','score_a37.py'),('probe_source_sha256','probe.rs'),('launcher_sha256','launch.sh')):
        assert registration['identities'][identity]==sha256(files[name]).hexdigest()
    assert registration['qualification_sha256']==sha256(files['QUAL.SHA256']).hexdigest()
    for cell,want in Q.items():
        match_sum(registration['frozen_values'][cell],want)
    schedule=[]
    for cell in Q:
        for i, (order, budgets) in enumerate(zip(ORDER,BUDGET_ORDER,strict=True),1):
            for k,budget in enumerate(budgets,1):
                schedule.append({'id':f'{len(schedule)+1:02d}-{cell}-r{i}-{budget}','cell':cell,'store':'lubm1000',
                  'rep':i,'order':order,'trace':True,'budget':budget,'budget_mb':BUDGETS[budget],'budget_position':k})
    assert schedule==registration['phases']
    status=read_json(ATTEMPT+'status.json')
    assert set(status)=={p['id'] for p in schedule}
    all_passes=[]
    for phase in schedule:
        prefix=ATTEMPT+phase['id']+'/'
        all_passes+=check_phase(phase, read_json(prefix+'out/probe.json'), read_json(prefix+'receipt.json'),
                                read_json(prefix+'inspect.json'),status[phase['id']],files)
    observed=defaultdict(list)
    for record in all_passes:
        observed[f'{record["cell"]} {record["budget"]}'].append(record)
    author=read_json('SUMMARY.json')
    author_passes={(r['phase'],r['pass']):r for r in author['passes']}
    assert len(author_passes)==len(author['passes'])==len(all_passes)==60
    cells={}
    for title,rows in sorted(observed.items()):
        assert len(rows)==10 and len({row['phase'] for row in rows})==5
        metrics={k:table([r['slot7'][k] for r in rows]) for k in COUNTERS}
        lookup_counts={r['slot7']['hits']+r['slot7']['misses'] for r in rows}
        assert len(lookup_counts)==1 and all(x[0]==x[2] for x in metrics.values())
        control=[r for r in rows if r['pass']=='control']
        prof=[r for r in rows if r['pass']=='profiled']
        assert len(control)==len(prof)==5
        conversion=lambda r:r['sysdelta']['convert_window']
        peak={r['phase']:r['peak_bytes'] for r in rows}
        actual={'budget_mb':BUDGETS[rows[0]['budget']],
                'dict_by_id_shard_budget_bytes':BUDGETS[rows[0]['budget']]*2**20//3//16,
                'passes':len(rows),'slot7_min_med_max':metrics,
                'other_slots_moved_in_convert':sorted({i for r in rows for i in r['other_slots']}),
                'control_total_ms':table([r['conversion_total_ms'] for r in control]),
                'profiled_total_ms':table([r['conversion_total_ms'] for r in prof]),
                'profiled_decode_ms':table([r['decode_ms'] for r in prof]),
                'ids_ms':table([r['ids_ms'] for r in rows]),
                'conv_read_bytes':table([conversion(r)['proc_io']['read_bytes'] for r in rows]),
                'conv_pgmajfault':table([conversion(r)['memory_stat']['pgmajfault'] for r in rows]),
                'peak_GiB':table([round(value/2**30,3) for value in peak.values()])}
        assert actual==author['cells'][title]
        for r in rows:
            peer=author_passes[(r['phase'],r['pass'])]
            assert (peer['position'],peer['budget_mb'],peer['slot7'],peer['other_slots_moved']) == (
                r['position'],actual['budget_mb'],r['slot7'],r['other_slots'])
            assert peer['query_slot_totals']=={str(i):d for i,d in enumerate(r['query_slots']) if any(d.values())}
            assert (peer['ids_ms'],peer['total_ms'],peer['decode_ms'],peer['peak_bytes'])==(
                r['ids_ms'],r['conversion_total_ms'],r['decode_ms'],r['peak_bytes'])
            assert (peer['conv_read_bytes'],peer['conv_rchar'],peer['conv_pgmajfault'])==(
                conversion(r)['proc_io']['read_bytes'],conversion(r)['proc_io']['rchar'],
                conversion(r)['memory_stat']['pgmajfault'])
        cells[title]=dict(actual, phases=len(peak), lookup_count=lookup_counts.pop(),
            budget_positions=dict(sorted(Counter(r['budget_position'] for r in control).items())),
            control_ids_plus_conversion_ms=table([r['ids_ms']+r['conversion_total_ms'] for r in control]),
            cgroup_peak_bytes=table(list(peak.values())))
    foreign=[]
    for label in ('before','after'):
        name=f'foreign-cpu-{label}-scored-v2.txt'
        stats={line.split()[0]:line.split()[1] for line in files[name].decode().splitlines()
               if line.startswith('tp-live-swe')}
        foreign.append(stats)
    result={'registration':REGISTRATION,'archived_manifest_members':len(files),
            'unmanifested_appledouble_metadata_members':len(metadata),'phases':30,'measured_passes':60,
            'window_deltas':120,'locality_phases':15,'answer_summaries':105,
            'conversion_read_bytes_histogram':dict(sorted(Counter(
                r['sysdelta']['convert_window']['proc_io']['read_bytes'] for r in all_passes).items())),
            'outside_cpu_snapshots_tp_live_swe':foreign,'outside_name_overlap':len(set(foreign[0])&set(foreign[1])),
            'summary_matches':True,'cells':cells}
    print(json.dumps(result,indent=2,sort_keys=True))


if __name__=='__main__':
    main()
