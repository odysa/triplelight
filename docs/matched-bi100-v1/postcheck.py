"""Replay retained BI100M observations; never changes a gate or input."""
import collections,hashlib,json,sys
from pathlib import Path
stage=Path(__file__).resolve().parent;root=stage.parent;out=stage/'round1'
sys.path.insert(0,str(stage/'bench-source'))
from http_measure import verify
from http_measure_value import check_policy
from envelope_qualify import FrozenInputs
from count_equivalence import normalize,load_freeze
from value_diagnostic import diagnose
from qualify import exact_term
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
protocol=json.loads((stage/'protocol.json').read_text())
assert sha(stage/'protocol.json')=='de7f1a84d4343494d4e149e3245caa423f024a9530d63532539383fabfeb5ff9'
manifest=json.loads((stage/'workload/manifest.json').read_text())
assert sha(stage/'workload/manifest.json')==protocol['workload_manifest_sha256']
reference=root/'bsbm100-reference-v1'
policy=check_policy(reference/'value-policy-freeze.json',protocol['gates']['value_policy'],protocol['workload_manifest_sha256'])
load_freeze(reference/'count-policy-freeze.json',protocol['gates']['count_policy'])
frozen=FrozenInputs(root/'numeric-freeze-100m-v1',protocol['gates']['numeric_freeze'],protocol['dataset_sha256'],protocol['workload_manifest_sha256'])
for name,digest in json.loads((stage/'runtime-source-hashes.json').read_text()).items():assert sha(stage/'bench-source'/name)==digest,name
assert sha(stage/'target-linux-ef7a778/release/rdfdb')==json.loads((stage/'rdfdb-linux-binary.json').read_text())['sha256']
summary={'schema':'matched-bi100-postaudit-v1','phases':{},'all_planned_phases_present':True}
def attempt(fn):
    try:fn();return True,None
    except (ValueError,KeyError,OverflowError) as exc:return False,type(exc).__name__+': '+str(exc)
def bag(result):return collections.Counter(tuple((k,exact_term(v)) for k,v in sorted(row.items())) for row in result['results']['bindings'])
for passno,systems in enumerate(protocol['passes'],1):
    for system in systems:
        name=f'pass{passno}-{system}';phase=out/name;path=phase/'queries/measurements.json'
        result={'system':system,'pass':passno,'queries':{},'complete':False}
        summary['phases'][name]=result
        if not path.exists():summary['all_planned_phases_present']=False;result['error']='Missing measurement report';continue
        report=json.loads(path.read_text())
        assert sha(phase/'runtime-evidence.json')==report['runtime_evidence_sha256']
        assert report['numeric_freeze_sha256']==protocol['gates']['numeric_freeze']
        assert report['count_equivalence_freeze_sha256']==protocol['gates']['count_policy']
        assert report['value_policy_sha256']==protocol['gates']['value_policy']
        for q in manifest['queries']:
            n=q['template'];stem=Path(q['file']).stem
            assert sha(stage/'workload/queries'/q['file'])==q['sha256']
            entry=report['queries'].get(q['file'])
            row={'primary':False,'count':False,'value':False,'exact_term_bag':False}
            result['queries'][q['file']]=row
            if entry is None:row['error']='Missing query entry';continue
            assert entry['mix']==q['mix'] and entry['template']==n and entry['scored']==(q['mix'] in [1,2,3])
            if entry['status']=='not_run':row['error']='Not run';continue
            body=phase/'queries'/(stem+'.body')
            if 'body_sha256' not in entry:row['error']='No main response after transport/control failure';continue
            assert sha(body)==entry['body_sha256']
            row['body_sha256']=sha(body)
            if entry.get('transport_error') or entry.get('http_status')!=200:
                row['error']='Transport failure';continue
            actual=json.loads(body.read_bytes());ref_path=reference/'reference'/(stem+'.json')
            assert sha(ref_path)==policy['reference_files'][ref_path.name]
            expected=json.loads(ref_path.read_bytes());diagnostic_path=reference/'reference'/(stem+'-diagnostic.json')
            d=json.loads(diagnostic_path.read_text()) if n==7 else None
            if n==7:assert sha(diagnostic_path)==policy['reference_files'][diagnostic_path.name]
            primary,primary_error=attempt(lambda:verify(n,actual,expected,d,frozen,q))
            count,count_error=attempt(lambda:verify(n,normalize(n,actual),normalize(n,expected),d,frozen,q))
            def value_check():
                if n==7:verify(n,actual,expected,d,frozen,q)
                else:
                    r=diagnose(n,actual,expected,frozen.for_instance(q) if n in [4,5,6] else None)
                    if r['order']['status']!='pass':raise ValueError('Value-compatible ordering mismatch')
            value,value_error=attempt(value_check)
            assert primary==(entry['status']=='pass')
            assert count==(entry.get('count_equivalence',{}).get('status')=='pass')
            assert value==(entry.get('value_compatibility',{}).get('status')=='pass')
            row.update(primary=primary,count=count,value=value,exact_term_bag=bag(actual)==bag(expected),
                primary_error=primary_error,count_error=count_error,value_error=value_error,reference_sha256=sha(ref_path))
            if system=='qlever':
                control=phase/'queries'/(stem+'-control.body')
                assert json.loads(control.read_bytes())==json.loads((phase/'initial-clear.json').read_text())
                row['cache_ack_sha256']=sha(control)
            assert isinstance(entry['elapsed_ns'],int) and entry['elapsed_ns']>=0
        result['complete']=report['complete'] and set(report['queries'])=={q['file'] for q in manifest['queries']}
        result['counts']={key:sum(q[key] for q in result['queries'].values()) for key in ['primary','count','value','exact_term_bag']}
        fields={'primary':'measured_total_ns','count':'integer_value_equivalent_total_ns','value':'value_compatible_total_ns'}
        for key,field in fields.items():
            eligible=result['complete'] and result['counts'][key]==60
            elapsed=sum(q['elapsed_ns'] for q in report['queries'].values() if q['scored']) if eligible else None
            assert report.get(field)==elapsed,(name,key,report.get(field),elapsed)
            result[key+'_total_ns']=elapsed
        if (phase/'container-state-before-stop.json').exists():result['state_before_stop']=json.loads((phase/'container-state-before-stop.json').read_text())
        if (phase/'container-state-after-stop.json').exists():result['state_after_stop']=json.loads((phase/'container-state-after-stop.json').read_text())
        cgroup=phase/'cgroup-memory.json'
        if cgroup.exists():
            resource=json.loads(cgroup.read_text());result['memory_scope']='container cgroup memory.peak, not process RSS'
            if resource['exit_code']==0:
                values=resource['stdout'].split();assert len(values)==3
                result['cgroup_peak_bytes']=int(values[0]);result['memory_max_bytes']=int(values[1]);result['swap_max_bytes']=int(values[2])
                assert result['memory_max_bytes']==protocol['container_memory_bytes'] and result['swap_max_bytes']==0
        samples=phase/'supervisor.samples.jsonl'
        if samples.exists():
            data=[json.loads(line) for line in samples.read_text().splitlines() if line]
            result['guard_samples']=len(data)
            result['sampled_max_root_bytes']=max(r['allocated_bytes'] for r in data)
            result['sampled_min_free_bytes']=min(r['free_bytes'] for r in data)
        print(name,result['counts'],result['value_total_ns'],flush=True)
before=json.loads((out/'input-hashes-before.json').read_text());after=json.loads((out/'input-hashes-after.json').read_text())
assert before==after
summary['inputs_and_sources_verified']=True
summary['both_passes_value_qualified']=summary['all_planned_phases_present'] and all(r.get('value_total_ns') is not None for r in summary['phases'].values())
summary['ratios_by_pass']={}
if summary['both_passes_value_qualified']:
    for passno in [1,2]:
        values={system:summary['phases'][f'pass{passno}-{system}']['value_total_ns'] for system in ['rdfdb','duckdb','qlever']}
        summary['ratios_by_pass'][str(passno)]={'qlever_over_rdfdb':values['qlever']/values['rdfdb'],'duckdb_over_rdfdb':values['duckdb']/values['rdfdb']}
(out/'postchecks.json').write_text(json.dumps(summary,indent=2))
print(summary['ratios_by_pass'])
