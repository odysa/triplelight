"""Exhaustive Jena-only q1 integer-COUNT reference, not a timing implementation.

Every product-type group belongs to one partition. With at most eight types
per partition, the unchanged LIMIT 10 cannot drop a group. Union all groups,
then apply the original descending-count / ascending-IRI order and LIMIT 10.
"""
import re

TYPE_PATTERN=r'(?m)^(\s*)\?productType a bsbm:ProductType \.$'
XSD_INTEGER='http://www.w3.org/2001/XMLSchema#integer'

def partition_query(source,types):
    if not 1<=len(types)<=8 or len(set(types))!=len(types):
        raise ValueError('One to eight distinct types required')
    if len(re.findall(TYPE_PATTERN,source))!=1 or not source.rstrip().endswith('Limit 10'):
        raise ValueError('Unsupported q1 source shape')
    terms=[]
    for value in types:
        if not value.startswith(('http://','https://')) or any(ord(c)<33 or ord(c)>126 or c in '<>"{}|^`\\' for c in value):
            raise ValueError('Unsupported product-type IRI')
        terms.append('<'+value+'>')
    values='\n VALUES ?productType { '+' '.join(terms)+' }'
    return re.sub(TYPE_PATTERN,lambda m:m.group(0)+values,source,count=1)

def assemble(results,expected_types):
    results=list(results)
    expected=set(expected_types)
    if len(expected)!=len(expected_types):raise ValueError('Duplicate partition domain')
    rows=[];seen=set()
    for requested,result in results:
        domain=set(requested)
        if not domain<=expected:raise ValueError('Partition escaped domain')
        if result['head']['vars']!=['productType','reviewCount']:raise ValueError('Unexpected projection')
        for row in result['results']['bindings']:
            if set(row)!={'productType','reviewCount'}:raise ValueError('Unexpected binding shape')
            kind=row['productType'];count=row['reviewCount']
            if kind.get('type')!='uri' or kind['value'] not in domain or kind['value'] in seen:
                raise ValueError('Unexpected or duplicate group')
            if count.get('type')!='literal' or count.get('datatype')!=XSD_INTEGER or not re.fullmatch(r'[+]?[0-9]+',count['value']) or int(count['value'])<=0:
                raise ValueError('Invalid grouped review COUNT')
            seen.add(kind['value']);rows.append(row)
    # Domain coverage includes empty groups, whose partition responses have no row.
    all_requested=[value for requested,_ in results for value in requested]
    if len(all_requested)!=len(expected) or set(all_requested)!=expected:
        raise ValueError('Incomplete or overlapping partition domain')
    rows.sort(key=lambda r:(-int(r['reviewCount']['value']),r['productType']['value']))
    return {'head':{'vars':['productType','reviewCount']},'results':{'bindings':rows[:10]}}
