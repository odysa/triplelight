"""Retrospective value-only BI diagnostic; never replaces frozen qualification.

Decode decimal/float/double according to their own value spaces, without casting
higher precision outputs down to binary32. Use the original rational envelopes
for q4/5/6 and the original +/-4 binary32-step interval around Jena for q3/8.
All other term identities, binding state, multiplicities and ordering remain.
"""
from collections import Counter, defaultdict
from fractions import Fraction
import math
import re
import struct

from envelope_qualify import anchor
from float_envelope import round_binary32
from qualify import XSD, FLOAT_COLUMNS, COUNT_COLUMNS, ORDER_KEYS, exact_term

DECIMAL = r'[+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)'
REAL = DECIMAL + r'(?:[eE][+-]?[0-9]+)?'
INTEGER_TYPES = {'integer', 'int', 'long', 'short', 'byte', 'nonNegativeInteger',
                 'positiveInteger', 'nonPositiveInteger', 'negativeInteger',
                 'unsignedLong', 'unsignedInt', 'unsignedShort', 'unsignedByte'}
RANGES = {'byte': (-128,127), 'short': (-2**15,2**15-1), 'int': (-2**31,2**31-1),
          'long': (-2**63,2**63-1), 'unsignedByte': (0,255),
          'unsignedShort': (0,2**16-1), 'unsignedInt': (0,2**32-1),
          'unsignedLong': (0,2**64-1)}


def numeric_value(term):
    if term is None or term.get('type') not in ('literal', 'typed-literal'):
        raise ValueError('Numeric output must be a bound typed literal')
    if term.get('xml:lang') or term.get('lang'): raise ValueError('Numeric literal cannot have a language tag')
    dt = term.get('datatype', '')
    if not dt.startswith(XSD): raise ValueError('Unsupported numeric datatype')
    kind = dt[len(XSD):]; text = term['value'].strip(' \t\r\n')
    if kind in INTEGER_TYPES:
        if not re.fullmatch(r'[+-]?[0-9]+',text): raise ValueError('Invalid integer lexical form')
        value = int(text)
        if kind in RANGES and not RANGES[kind][0] <= value <= RANGES[kind][1]:
            raise ValueError('Integer outside declared datatype range')
        if ((kind == 'nonNegativeInteger' and value < 0) or
            (kind == 'positiveInteger' and value <= 0) or
            (kind == 'nonPositiveInteger' and value > 0) or
            (kind == 'negativeInteger' and value >= 0)):
            raise ValueError('Integer outside declared datatype range')
        return Fraction(value)
    if kind not in ('decimal', 'float', 'double'):
        raise ValueError('Unsupported numeric datatype')
    if not re.fullmatch(DECIMAL if kind == 'decimal' else REAL,text):
        raise ValueError('Invalid or unsupported non-finite numeric lexical form')
    rational = Fraction(text)
    if kind == 'decimal': return rational
    if kind == 'float': return round_binary32(rational)
    value = float(rational)  # Python converts rational to nearest/ties-even binary64.
    if not math.isfinite(value): raise ValueError('Non-finite double is unsupported')
    return Fraction.from_float(value)


def four_step_interval(reference):
    value = numeric_value(reference)
    if reference.get('datatype') != XSD+'float':
        raise ValueError('Frozen q3/q8 reference must be xsd:float')
    bits = struct.unpack('>I', struct.pack('>f',float(value)))[0]
    ordered = (~bits & 0xffffffff) if bits & 0x80000000 else bits | 0x80000000
    def decode(key):
        raw = (~key & 0xffffffff) if key < 0x80000000 else key & 0x7fffffff
        x = struct.unpack('>f',struct.pack('>I',raw))[0]
        if not math.isfinite(x): raise ValueError('Unbounded four-step interval')
        return Fraction.from_float(x)
    return decode(ordered-4), decode(ordered+4)


def order_diagnostic(number, actual):
    previous = None
    for row in actual['results']['bindings']:
        current=[]
        for variable, descending in ORDER_KEYS.get(number,[]):
            term=row.get(variable)
            if term is None: value=(0,None)
            elif variable in FLOAT_COLUMNS.get(number,set()) | COUNT_COLUMNS.get(number,set()):
                value=(1,numeric_value(term))
            else:
                if term.get('type') != 'uri': raise ValueError('Expected URI ordering key')
                value=(1,term['value'])
            current.append(value)
        if previous is not None:
            for a,b,(_,descending) in zip(previous,current,ORDER_KEYS.get(number,[])):
                if a==b: continue
                if (a>b) != descending: raise ValueError('Outer ordering is not monotonic')
                break
        previous=current
    return {'checked': number in ORDER_KEYS}


def diagnose(number,actual,reference,inputs=None):
    variables=reference['head']['vars']
    if len(set(variables)) != len(variables) or len(actual['head']['vars']) != len(variables) or set(variables)!=set(actual['head']['vars']):
        raise ValueError('Projected variables differ')
    floats=FLOAT_COLUMNS.get(number,set()); counts=COUNT_COLUMNS.get(number,set())
    def groups(result):
        out=defaultdict(list)
        for row in result['results']['bindings']:
            key=[]; values=[]
            for variable in variables:
                term=row.get(variable)
                if variable in floats: values.append(term)
                elif variable in counts and term is not None:
                    value=numeric_value(term)
                    if value.denominator != 1 or value < 0: raise ValueError('COUNT must be an exact nonnegative integer value')
                    key.append(('count',value))
                else: key.append(exact_term(term))
            out[tuple(key)].append(values)
        return out
    a,b=groups(actual),groups(reference)
    if Counter({k:len(v) for k,v in a.items()}) != Counter({k:len(v) for k,v in b.items()}):
        raise ValueError('Entity/count result bags differ')
    if number in (4,5,6):
        entity_variable={4:'feature',5:'product',6:'reviewer'}[number]
        if inputs is None: raise ValueError('Missing frozen numerical inputs')
        for role,result in [('reference',reference),('candidate',actual)]:
            for row in result['results']['bindings']:
                entity=row[entity_variable]['value']
                if entity not in inputs['groups']: raise ValueError('Unknown input group')
                bounds,_=anchor(number,inputs['groups'][entity])
                for variable in floats:
                    if not bounds.contains(numeric_value(row.get(variable))):
                        raise ValueError(role+' numeric value outside frozen exact-anchor envelope')
    elif floats:
        # Each supported template has one computed numeric output per row.
        if len(floats)!=1: raise ValueError('Unsupported multivariate numeric matching')
        for key,rows in a.items():
            left=sorted((numeric_value(r[0]) for r in rows if r[0] is not None))
            right=[r[0] for r in b[key] if r[0] is not None]
            if len(left)!=len(right): raise ValueError('Numeric bound/unbound state differs')
            right.sort(key=numeric_value)
            for value,ref in zip(left,right):
                lo,hi=four_step_interval(ref)
                if not lo<=value<=hi: raise ValueError('Numeric value outside frozen four-step reference interval')
    try:
        order={'status':'pass',**order_diagnostic(number,actual)}
    except ValueError as exc:
        order={'status':'fail','error':str(exc)}
    return {'rows':len(actual['results']['bindings']), 'value_bag':'pass', 'order':order}
