"""BI qualification: all output columns, numeric types/ULPs and q7 cutoff ties.

The four-ULP computed-float allowance is declared before database measurements.
Exact representation equality is reported separately. This is not a tolerance
for sourced RDF literals, identifiers, integer counts or datatype mismatches.
"""
from collections import Counter, defaultdict
import math
import struct

XSD='http://www.w3.org/2001/XMLSchema#'
FLOAT_COLUMNS={3:{'ratio'},4:{'priceRatio'},5:{'avgPrice'},6:{'reviewerAvgScore'},8:{'cheapExpensiveRatio'}}
COUNT_COLUMNS={1:{'reviewCount'},2:{'sameFeatures'},5:{'nrOfReviews'}}
ORDER_KEYS={1:[('reviewCount',True),('productType',False)],
            2:[('sameFeatures',True),('otherProduct',False)],
            3:[('ratio',True),('product',False)],4:[('priceRatio',True),('feature',False)],
            5:[('nrOfReviews',True),('country',False),('product',False)],
            8:[('cheapExpensiveRatio',True),('vendor',False)]}


def exact_term(value):
    if value is None:return None
    kind=value['type'];kind='literal' if kind=='typed-literal' else kind
    dt=value.get('datatype');lang=value.get('xml:lang')
    if dt==XSD+'string':dt=None
    if lang:lang=lang.lower();dt=None
    return kind,value['value'],dt,lang


def as_float(term):
    if term is None:return None
    if term['type'] not in ('literal','typed-literal') or term.get('datatype')!=XSD+'float':
        raise ValueError('Computed float output must have xsd:float datatype')
    x=float(term['value'])
    try:return struct.unpack('>f',struct.pack('>f',x))[0]
    except OverflowError:return math.copysign(math.inf,x)


def ulps(a,b):
    if math.isnan(a) or math.isnan(b):return 0 if math.isnan(a) and math.isnan(b) else math.inf
    if a==b:return 0
    if not math.isfinite(a) or not math.isfinite(b):return math.inf
    def ordered(v):
        bits=struct.unpack('>I',struct.pack('>f',v))[0]
        return (~bits & 0xffffffff) if bits & 0x80000000 else bits|0x80000000
    return abs(ordered(a)-ordered(b))


def qualify(number, actual, reference, max_ulps=4):
    variables=reference['head']['vars']
    if set(variables)!=set(actual['head']['vars']):raise ValueError('Projected variables differ')
    floats=FLOAT_COLUMNS.get(number,set());counts=COUNT_COLUMNS.get(number,set())
    def groups(result):
        out=defaultdict(list)
        for row in result['results']['bindings']:
            key=[];values=[]
            for v in variables:
                value=row.get(v)
                if v in floats:values.append(as_float(value))
                elif v in counts and value is not None:
                    if value.get('datatype')!=XSD+'integer':raise ValueError('Count datatype mismatch')
                    key.append(('integer',int(value['value'])))
                else:key.append(exact_term(value))
            out[tuple(key)].append(tuple(values))
        return out
    a,b=groups(actual),groups(reference)
    if a.keys()!=b.keys():raise ValueError('Entity/count result bags differ')
    worst=0
    for key,values in a.items():
        expected=list(b[key])
        for row in values:
            for i,ref in enumerate(expected):
                distances=[0 if x is y is None else math.inf if x is None or y is None else ulps(x,y) for x,y in zip(row,ref)]
                distance=max(distances,default=0)
                if distance<=max_ulps:
                    worst=max(worst,distance);expected.pop(i);break
            else:raise ValueError('Computed numeric values or multiplicities differ')
        if expected:raise ValueError('Result multiplicities differ')
    def exact(result):
        return Counter(tuple(exact_term(row.get(v)) for v in variables) for row in result['results']['bindings'])
    return {'all_columns_compatible':True,'exact_term_bag':exact(actual)==exact(reference),
            'computed_float_max_ulps':worst,'computed_float_allowed_ulps':max_ulps,
            'rows':len(actual['results']['bindings'])}


def check_order(number, result):
    if number not in ORDER_KEYS:return {'checked':False,'reason':'no outer ORDER BY'}
    def key(v,t):
        if t is None:return 0,None
        if v in FLOAT_COLUMNS.get(number,set()):
            value=as_float(t)
            if math.isnan(value):raise ValueError('NaN ordered key needs a separate oracle')
        elif v in COUNT_COLUMNS.get(number,set()):value=int(t['value'])
        else:
            if t['type']!='uri':raise ValueError('Expected URI secondary ordering key')
            value=t['value']
        return 1,value
    previous=None
    for row in result['results']['bindings']:
        current=[key(v,row.get(v)) for v,_ in ORDER_KEYS[number]]
        if previous is not None:
            for a,b,(_,descending) in zip(previous,current,ORDER_KEYS[number]):
                if a==b:continue
                if (a>b)!=descending:raise ValueError('BI outer ordering is not monotonic')
                break
        previous=current
    return {'checked':True,'keys':ORDER_KEYS[number]}


def q7_allowed_products(actual_products, diagnostic_rows, limit=1000):
    """Oracle covers the inner LIMIT before the outer NOT EXISTS filter.

    diagnostic_rows: unique product, offerCount, blocked (has a vendor in country).
    Boundary products may be chosen arbitrarily to fill the remaining slots.
    """
    if not isinstance(limit,int) or limit<1:raise ValueError('Positive integer cutoff required')
    if any(type(row['blocked']) is not bool or type(row['offerCount']) is not int for row in diagnostic_rows):
        raise ValueError('Diagnostic blocked/count fields must be decoded typed bool/int values')
    products={row['product']:row for row in diagnostic_rows}
    if len(products)!=len(diagnostic_rows):raise ValueError('Diagnostic products must be unique')
    if len(actual_products)!=len(set(actual_products)):raise ValueError('Duplicate q7 product')
    if any(int(r['offerCount'])<1 for r in diagnostic_rows):raise ValueError('Invalid offer count')
    if len(products)<=limit:
        expected={p for p,r in products.items() if not r['blocked']}
        if set(actual_products)!=expected:raise ValueError('q7 full eligible set differs')
        return {'tie_aware':True,'ambiguous_boundary':False,'rows':len(actual_products)}
    cutoff=sorted((int(r['offerCount']) for r in products.values()),reverse=True)[limit-1]
    above={p for p,r in products.items() if int(r['offerCount'])>cutoff}
    boundary={p for p,r in products.items() if int(r['offerCount'])==cutoff}
    mandatory={p for p in above if not products[p]['blocked']}
    eligible={p for p in boundary if not products[p]['blocked']}
    actual=set(actual_products)
    if not mandatory<=actual or not actual<=mandatory|eligible:
        raise ValueError('q7 membership outside allowed cutoff selection')
    slots=limit-len(above);rejected=len(boundary)-len(eligible)
    low=max(0,slots-rejected);high=min(slots,len(eligible));chosen=len(actual-mandatory)
    if not low<=chosen<=high:raise ValueError('q7 cutoff tie cardinality is impossible')
    return {'tie_aware':True,'ambiguous_boundary':len(boundary)>slots,
            'cutoff_offer_count':cutoff,'boundary_slots':slots,'boundary_products':len(boundary),
            'allowed_optional_rows':[low,high],'rows':len(actual_products)}
