#!/usr/bin/env python3
"""Prepare Jena references or qualify a CLI binary; retain every process result."""
import argparse, collections, hashlib, json, os, subprocess, sys
from pathlib import Path
XSD_STRING = 'http://www.w3.org/2001/XMLSchema#string'
def sha(p): return hashlib.sha256(Path(p).read_bytes()).hexdigest()
def term(t):
    if t is None: return ('unbound',)
    kind=t['type']
    if kind=='uri': return ('uri',t['value'])
    if kind in ('literal','typed-literal'):
        lang=t.get('xml:lang',t.get('lang','')).lower()
        return ('literal',t['value'],lang,t.get('datatype','') if not lang else '') if lang else ('literal',t['value'],'',t.get('datatype',XSD_STRING))
    if kind=='bnode': raise ValueError('Projected blank nodes require isomorphism comparison; outside this suite')
    raise ValueError('Unknown RDF term kind '+str(kind))
def canonical(obj):
    if 'boolean' in obj:
        if not isinstance(obj['boolean'],bool): raise ValueError('ASK result is not a boolean')
        return ('boolean',obj['boolean'])
    variables=obj['head']['vars']
    if len(set(variables))!=len(variables): raise ValueError('Duplicate projected variable')
    rows=[]
    for row in obj['results']['bindings']:
        if set(row)-set(variables): raise ValueError('Binding outside projection')
        rows.append(tuple((v,term(row.get(v))) for v in sorted(variables)))
    return ('select',tuple(sorted(variables)),rows)
def compare(actual,reference,ordered=False):
    a,b=canonical(actual),canonical(reference)
    if a[0]!='select' or b[0]!='select': return a==b
    return a[:2]==b[:2] and (a[2]==b[2] if ordered else collections.Counter(a[2])==collections.Counter(b[2]))
def invoke(command,stdout,stderr,timeout,env=None):
    try:
        r=subprocess.run(command,capture_output=True,timeout=timeout,env=env);stdout.write_bytes(r.stdout);stderr.write_bytes(r.stderr)
        return {'command':command,'exit_code':r.returncode,'stdout_sha256':sha(stdout),'stderr_sha256':sha(stderr),'timed_out':False}
    except subprocess.TimeoutExpired as e:
        stdout.write_bytes(e.stdout or b'');stderr.write_bytes(e.stderr or b'')
        return {'command':command,'exit_code':None,'stdout_sha256':sha(stdout),'stderr_sha256':sha(stderr),'timed_out':True}
def main():
    ap=argparse.ArgumentParser(description=__doc__);ap.add_argument('--suite',type=Path,required=True);ap.add_argument('--output',type=Path,required=True)
    ap.add_argument('--jena-home',type=Path);ap.add_argument('--references',type=Path);ap.add_argument('--candidate',type=Path);ap.add_argument('--candidate-source');ap.add_argument('--timeout',type=int,default=45)
    args=ap.parse_args()
    if bool(args.jena_home)==bool(args.references):ap.error('Choose exactly one of --jena-home or --references')
    if args.candidate and not args.candidate_source:ap.error('--candidate-source is required with --candidate')
    suite_path=args.suite.resolve();base=suite_path.parent;suite=json.loads(suite_path.read_text());output=args.output.resolve();output.mkdir(parents=True,exist_ok=False)
    identity={'suite_sha256':sha(suite_path),'candidate_source':args.candidate_source,'candidate_binary_sha256':sha(args.candidate) if args.candidate else None,'references':{},'inputs':{}}
    for rel,digest in suite['fixtures'].items():
        assert sha(base/rel)==digest,rel;identity['inputs'][rel]=digest
    def save():
        (output/'identity.json').write_text(json.dumps(identity,indent=2)+'\n');(output/'results.json').write_text(json.dumps(results,indent=2)+'\n')
    results={};refs={}
    if args.references:
        binding=json.loads((args.references/'identity.json').read_text());assert binding['suite_sha256']==identity['suite_sha256']
    for c in suite['cases']:
        name=c['name'];assert sha(base/c['query'])==c['query_sha256'];identity['inputs'][c['query']]=c['query_sha256'];record={};results[name]=record
        if args.jena_home:
            cmd=[str(args.jena_home.resolve()/'bin/arq'),'--data='+str(base/c['fixture']),'--query='+str(base/c['query']),'--results=JSON']
            ref_file=output/(name+'-jena.json');record['oracle_process']=invoke(cmd,ref_file,output/(name+'-jena.stderr'),args.timeout,{**os.environ,'JVM_ARGS':'-Xmx1g -XX:ActiveProcessorCount=1'})
            if record['oracle_process']['exit_code']!=0:record['oracle_pass']=False;save();continue
        else:
            ref_file=args.references/(name+'-jena.json');assert sha(ref_file)==binding['references'][name]
        identity['references'][name]=sha(ref_file)
        try:
            reference=json.loads(ref_file.read_text());canonical(reference)
            assert 'expected_rows' not in c or len(reference['results']['bindings'])==c['expected_rows']
            assert 'expected_boolean' not in c or reference['boolean']==c['expected_boolean']
            if name=='sequence-count': assert reference['results']['bindings'][0]['n']['value']=='2'
            if name=='blank-node-intermediate': assert reference['results']['bindings'][0]['n']['value']=='1'
            record['oracle_pass']=True;record['reference_rows']=len(reference.get('results',{}).get('bindings',[])) if 'results' in reference else None
        except Exception as e:record['oracle_pass']=False;record['oracle_error']=repr(e);save();continue
        if args.candidate:
            candidate=output/(name+'-candidate.json');cmd=[str(args.candidate.resolve()),'query',str(base/c['fixture']),(base/c['query']).read_text()]
            record['candidate_process']=invoke(cmd,candidate,output/(name+'-candidate.stderr'),args.timeout,{**os.environ,'RAYON_NUM_THREADS':'1'})
            try:
                assert record['candidate_process']['exit_code']==0,'candidate process failed'
                actual=json.loads(candidate.read_text());record['candidate_pass']=compare(actual,reference,c['ordered'])
            except Exception as e:record['candidate_pass']=False;record['candidate_error']=repr(e)
        save()
    # Recheck immutable fixtures, queries, binary and references after all requests.
    assert sha(suite_path)==identity['suite_sha256']
    for rel,digest in identity['inputs'].items():assert sha(base/rel)==digest
    for name,digest in identity['references'].items():assert sha((args.references or output)/(name+'-jena.json'))==digest
    if args.candidate:assert sha(args.candidate)==identity['candidate_binary_sha256']
    oracle_gate=len(results)==len(suite['cases']) and all(r.get('oracle_pass') for r in results.values())
    candidate_gate=oracle_gate and all(r.get('candidate_pass') for r in results.values()) if args.candidate else None
    summary={'complete':True,'cases':len(results),'oracle_pass':sum(bool(r.get('oracle_pass')) for r in results.values()),'oracle_gate':oracle_gate,'candidate_pass':sum(bool(r.get('candidate_pass')) for r in results.values()) if args.candidate else None,'candidate_gate':candidate_gate,'inputs_unchanged':True,'comparison':'exact RDF term bags; explicit ordered case also exact sequence; no numerical tolerance'}
    (output/'summary.json').write_text(json.dumps(summary,indent=2)+'\n');save();print(json.dumps(summary,indent=2))
    return 0 if oracle_gate and candidate_gate is not False else 1
if __name__=='__main__':sys.exit(main())
