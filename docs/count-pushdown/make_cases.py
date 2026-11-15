#!/usr/bin/env python3
"""Generate exact-count cases only from the saved pure SELECT/BGP benchmark cohort."""
import re,json,hashlib
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
OUT=Path(__file__).resolve().parent/'queries'
references={}
for family,dataset in [('lubm','lubm100'),('watdiv','watdiv100')]:
    target=OUT/family
    target.mkdir(parents=True,exist_ok=True)
    source_counts=json.loads((ROOT/'bench/results'/dataset/'rdfdb-disk.json').read_text())
    refs={}
    for path in sorted((ROOT/'bench'/family).glob('*.rq')):
        text=path.read_text()
        match=re.search(r'\bSELECT\b(.*?)\bWHERE\b',text,re.I|re.S)
        assert match,path
        assert not re.search(r'\b(DISTINCT|REDUCED|GROUP|LIMIT|OFFSET|ORDER|FILTER|OPTIONAL|UNION)\b',text,re.I),path
        first=re.search(r'\?[A-Za-z_][A-Za-z_0-9]*',match.group(1))
        assert first,path
        for kind,expr in [('all','COUNT(*)'),('distinct','COUNT(DISTINCT '+first[0]+')')]:
            name=kind+'_'+path.stem
            query=text[:match.start()]+'SELECT ('+expr+' AS ?count_result) WHERE'+text[match.end():]
            (target/(name+'.rq')).write_text(query)
            if kind=='all':refs[name]=source_counts[path.stem]['rdfdb-disk']['rows']
    references[family]=refs
# Small-index-prefix grouping and repeated-variable fallback cases.
extra={
'lubm':{
 'group_advisors':'SELECT ?o (COUNT(*) AS ?n) (COUNT(DISTINCT ?s) AS ?d) WHERE {?s <http://swat.cse.lehigh.edu/onto/univ-bench.owl#advisor> ?o} GROUP BY ?o',
 'group_type':'SELECT ?o (COUNT(*) AS ?n) WHERE {?s <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> ?o} GROUP BY ?o',
},
'watdiv':{
 'group_age':'SELECT ?o (COUNT(*) AS ?n) (COUNT(DISTINCT ?s) AS ?d) WHERE {?s <http://xmlns.com/foaf/age> ?o} GROUP BY ?o',
 'group_gender':'SELECT ?o (COUNT(*) AS ?n) WHERE {?s <http://db.uwaterloo.ca/~galuc/wsdbm/gender> ?o} GROUP BY ?o',
}}
for family,cases in extra.items():
    for name,text in cases.items():(OUT/family/(name+'.rq')).write_text(text+'\n')
(Path(__file__).resolve().parent/'count_star_references.json').write_text(json.dumps(references,indent=2)+'\n')
manifest={str(p.relative_to(OUT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in OUT.rglob('*.rq')}
(Path(__file__).resolve().parent/'query_hashes.json').write_text(json.dumps(manifest,indent=2)+'\n')
print('Generated',len(manifest),'queries; COUNT(*) references',sum(map(len,references.values())))
