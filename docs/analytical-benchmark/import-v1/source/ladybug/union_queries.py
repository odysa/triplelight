"""Narrow native translations for the two UNION shapes in the BSBM cohort.

Used only after separately validating the BSBM path; not a general translator.
"""
import re


def compile_union(query, manifest, resources, compile_plain, term_key):
    prefixes='\n'.join(re.findall(r'PREFIX\s+[\w-]+:\s*<[^>]+>',query,re.I))
    q=re.sub(r'PREFIX\s+[\w-]+:\s*<[^>]+>','',query,flags=re.I).strip()
    m=re.fullmatch(r'SELECT\s+(.*?)\s+WHERE\s*\{\s*\{(.*?)\}\s*UNION\s*\{(.*?)\}\s*\}\s*(.*)',q,re.S|re.I)
    if not m:raise ValueError('Unsupported UNION shape')
    projection=m[1].split()
    if projection==['?property','?hasValue','?isValueOf'] and not m[4].strip():
        outgoing=re.fullmatch(r'\s*<([^>]+)>\s+\?property\s+\?hasValue\s*\.?\s*',m[2])
        incoming=re.fullmatch(r'\s*\?isValueOf\s+\?property\s+<([^>]+)>\s*\.?\s*',m[3])
        if not outgoing or not incoming or outgoing[1]!=incoming[1]:raise ValueError('Unsupported variable-predicate UNION')
        ident=resources.get(term_key({'type':'uri','value':outgoing[1]}),-1)
        cypher=(f'MATCH (s)-[r]->(o) WHERE s.id = {ident} '
                'RETURN label(r) AS property, o.term AS hasValue, CAST(NULL AS STRING) AS isValueOf '
                f'UNION ALL MATCH (s)-[r]->(o) WHERE o.id = {ident} '
                'RETURN label(r) AS property, CAST(NULL AS STRING) AS hasValue, s.term AS isValueOf')
        reverse={rel['table']:pred for pred,base in manifest['predicates'].items() for rel in manifest['relationships'] if rel['table'] in (base+'R',base+'L')}
        return {'vars':['property','hasValue','isValueOf'],'cypher':cypher,'predicate_columns':['property'],'predicate_labels':reverse}
    if not projection or projection.pop(0).upper()!='DISTINCT':raise ValueError('Only DISTINCT shared-projection UNION is translated')
    token=r'(?:\?\w+|<[^>]+>|[\w-]+:[\w-]+|a)'
    triple=re.compile(rf'({token})\s+({token})\s+({token})\s*\.')
    branches=[triple.findall(m[i]) for i in (2,3)]
    shared=[t for t in branches[0] if t in branches[1]]
    bound={t for triple in shared for t in triple if t.startswith('?')}
    if not set(projection)<=bound:raise ValueError('UNION projection not entirely bound by shared triples')
    common=' '.join(' '.join(t)+' .' for t in shared)
    root=compile_plain(prefixes+' SELECT DISTINCT '+' '.join(projection)+' WHERE { '+common+' } '+m[4],manifest,resources)
    clauses=[]
    for i,body in enumerate((m[2],m[3])):
        if re.search(r'OPTIONAL|UNION',body,re.I):raise ValueError('Nested UNION/OPTIONAL not translated here')
        branch=compile_plain(prefixes+' SELECT '+' '.join(projection)+' WHERE { '+body+' }',manifest,resources)['cypher'].split(' RETURN ',1)[0]
        if ' WITH * WHERE ' in branch:
            before,conditions=branch.split(' WITH * WHERE ',1)
            branch=before+(' AND ' if ' WHERE ' in before else ' WHERE ')+conditions
        branch=re.sub(r'\bc_\d+\b',lambda t:f'b{i}_'+t[0],branch)
        clauses.append('EXISTS { '+branch+' }')
    before,ret=root['cypher'].split(' RETURN ',1)
    root['cypher']=before+' WITH * WHERE ('+' OR '.join(clauses)+') RETURN '+ret
    return root
