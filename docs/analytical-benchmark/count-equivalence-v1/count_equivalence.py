"""Opt-in COUNT-only integer value equivalence; preserves strict qualification.

This policy never changes raw responses or the numeric-envelope freeze. It
normalizes only registered computed COUNT columns in BI templates 1, 2 and 5.
Other RDF terms, float datatypes, counts, bags, ordering and cutoff rules retain
the existing checks. xsd:int must be in its actual signed 32-bit value space.
"""
import copy
import hashlib
import json
from pathlib import Path
import re

XSD = 'http://www.w3.org/2001/XMLSchema#'
COUNT_SCOPE = {1: ['reviewCount'], 2: ['sameFeatures'], 5: ['nrOfReviews']}
POLICY = 'bi-count-int-integer-equivalence-v1'


def normalize(number, result):
    result = copy.deepcopy(result)
    for row in result['results']['bindings']:
        for variable in COUNT_SCOPE.get(number, []):
            term = row.get(variable)
            if term is None:
                continue  # The unchanged oracle decides whether unbound is valid.
            datatype = term.get('datatype')
            if term.get('type') not in ('literal', 'typed-literal') or datatype not in (XSD+'int', XSD+'integer') or term.get('xml:lang'):
                raise ValueError('COUNT is not an int/integer literal')
            if not re.fullmatch(r'[+-]?[0-9]+', term['value']):
                raise ValueError('COUNT has invalid integer lexical form')
            value = int(term['value'])
            if value < 0:
                raise ValueError('COUNT cannot be negative')
            if datatype == XSD+'int' and value > 2147483647:
                raise ValueError('COUNT is outside xsd:int value space')
            term['type'] = 'literal'; term['datatype'] = XSD+'integer'; term['value'] = str(value)
    return result


def load_freeze(path, expected_sha256):
    raw = path.read_bytes()
    if hashlib.sha256(raw).hexdigest() != expected_sha256:
        raise ValueError('Count policy freeze hash differs')
    frozen = json.loads(raw)
    if frozen['policy'] != POLICY or frozen['count_columns'] != {str(k): v for k,v in COUNT_SCOPE.items()}:
        raise ValueError('Count equivalence policy/scope differs')
    for name, digest in frozen['sources'].items():
        if Path(name).name != name or hashlib.sha256((Path(__file__).parent/name).read_bytes()).hexdigest() != digest:
            raise ValueError('Count policy source identity differs: '+name)
    return hashlib.sha256(raw).hexdigest()
