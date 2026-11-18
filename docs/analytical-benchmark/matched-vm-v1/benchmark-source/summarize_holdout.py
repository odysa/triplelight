#!/usr/bin/env python3
"""Summarize a completed frozen BI correctness cohort; never emit speed scores."""
import argparse
from collections import defaultdict, Counter
from fractions import Fraction
import json
from pathlib import Path

from float_envelope import round_binary32
from qualify import ulps, exact_term


def summarize(run):
    manifest=json.loads((run/'source/workloads/bsbm10/manifest.json').read_text())
    metadata={q['file']:q for q in manifest['queries']}
    variants=json.loads((run/'variants.json').read_text())
    names=['jena','duckdb']+[v['name'] for v in variants]
    report={'schema':'frozen-bi-correctness-summary-v1','cohort':json.loads((run/'cohort.json').read_text()),
            'systems':{},'speed_score':None,'mix0_prior_results_reclassified':False}
    for name in names:
        matrix=json.loads((run/name/'matrix.json').read_text())
        if not matrix['complete']:raise ValueError('Incomplete matrix: '+name)
        if set(matrix['queries'])!=set(metadata):raise ValueError('Scheduled query coverage differs: '+name)
        section={'runtime_source':matrix['runtime_source'],'all_queries':len(metadata),'confirmatory_queries':0,
                 'confirmatory_pass':0,'exploratory_pass':0,'failures':[],'numeric_by_template':{}}
        precision=defaultdict(lambda:{'observations':0,'max_absolute_error':Fraction(0),
            'max_relative_error':Fraction(0),'zero_ideal_values':0,'max_ulps_vs_rounded_exact_ideal':0})
        for file,entry in matrix['queries'].items():
            query=metadata[file];confirmatory=query['mix'] in (1,2,3)
            if confirmatory:section['confirmatory_queries']+=1
            if entry['status']!='pass':section['failures'].append({'file':file,'confirmatory':confirmatory,'error':entry.get('error')});continue
            section['confirmatory_pass' if confirmatory else 'exploratory_pass']+=1
            if not confirmatory:continue
            for values in entry.get('qualification',{}).get('numeric_details',{}).values():
                stats=precision[query['template']];stats['observations']+=1
                ideal=Fraction(values['exact_ideal']);value=Fraction(values['candidate']['value']);error=abs(value-ideal)
                stats['max_absolute_error']=max(stats['max_absolute_error'],error)
                if ideal:stats['max_relative_error']=max(stats['max_relative_error'],error/abs(ideal))
                else:stats['zero_ideal_values']+=1
                stats['max_ulps_vs_rounded_exact_ideal']=max(stats['max_ulps_vs_rounded_exact_ideal'],
                    ulps(float(value),float(round_binary32(ideal))))
        for template,stats in precision.items():
            section['numeric_by_template'][str(template)]={
                **{k:v for k,v in stats.items() if not isinstance(v,Fraction)},
                'max_absolute_error_exact':str(stats['max_absolute_error']),
                'max_relative_error_exact':str(stats['max_relative_error']),
                'max_absolute_error_display':float(stats['max_absolute_error']),
                'max_relative_error_display':float(stats['max_relative_error'])}
        section['all_scheduled_queries_pass']=not section['failures']
        report['systems'][name]=section
    if {'rdfdb-main','rdfdb-main-legacy'} <= set(names):
        mismatches=[]
        for name in metadata:
            stem=Path(name).stem
            a=json.loads((run/'rdfdb-main'/(stem+'.json')).read_text())
            b=json.loads((run/'rdfdb-main-legacy'/(stem+'.json')).read_text())
            variables=sorted(a['head']['vars'])
            def bag(result):
                return Counter(tuple(exact_term(row.get(v)) for v in variables)
                               for row in result['results']['bindings'])
            if set(variables)!=set(b['head']['vars']) or bag(a)!=bag(b):mismatches.append(name)
        report['main_new_vs_general']={'comparison':'exact RDF-term bags, including numeric lexical forms',
                                      'queries':len(metadata),'mismatches':mismatches}
    report['all_scheduled_queries_pass']=all(s['all_scheduled_queries_pass'] for s in report['systems'].values())
    return report


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--run',type=Path,required=True);p.add_argument('--out',type=Path,required=True)
    args=p.parse_args()
    if args.out.exists():raise FileExistsError('Preserve prior summaries')
    result=summarize(args.run);args.out.write_text(json.dumps(result,indent=2))
    for name,entry in result['systems'].items():print(name,entry['confirmatory_pass'],'/',entry['confirmatory_queries'],'confirmatory; failures',len(entry['failures']))
