"""Exact integer COUNT compatibility for the separately curated aggregate cohort.

This is not the frozen BSBM BI policy. Only declared computed COUNT columns are
normalized; group keys retain RDF-term identity and rows retain multiplicity.
"""
from collections import Counter
import re

from qualify import exact_term

XSD = 'http://www.w3.org/2001/XMLSchema#'
COUNT_TYPES = {
    XSD + 'integer': (None, None),
    XSD + 'int': (-(2**31), 2**31 - 1),
    XSD + 'long': (-(2**63), 2**63 - 1),
}
INTEGER_LEXICAL = re.compile(r'[+-]?[0-9]+\Z')


def count_value(term):
    """Validate the declared integer datatype before interpreting a COUNT value."""
    if not isinstance(term, dict) or term.get('type') not in ('literal', 'typed-literal'):
        raise ValueError('COUNT must be an integer literal')
    datatype = term.get('datatype')
    if datatype not in COUNT_TYPES or term.get('xml:lang') is not None:
        raise ValueError('COUNT datatype is not in the registered integer family')
    lexical = term.get('value')
    if not isinstance(lexical, str):
        raise ValueError('COUNT lexical form must be a string')
    # XML Schema integer's whitespace facet collapses XML whitespace only.
    lexical = lexical.strip(' \t\r\n')
    if not INTEGER_LEXICAL.fullmatch(lexical):
        raise ValueError('Invalid integer COUNT lexical form')
    value = int(lexical)
    lower, upper = COUNT_TYPES[datatype]
    if value < 0 or (lower is not None and value < lower) or (upper is not None and value > upper):
        raise ValueError('COUNT is negative or outside its declared datatype range')
    return value


def projection(columns):
    columns = tuple(tuple(c) for c in columns)
    names = tuple(name for name, _ in columns)
    if not names or len(set(names)) != len(names):
        raise ValueError('Projection names must be nonempty and unique')
    if any(kind not in ('term', 'count') for _, kind in columns):
        raise ValueError('Only term keys and computed COUNT outputs are supported')
    if not any(kind == 'count' for _, kind in columns):
        raise ValueError('No computed COUNT column declared')
    return columns


def checked_rows(result, columns):
    names = [name for name, _ in columns]
    variables = result['head']['vars']
    if len(variables) != len(set(variables)) or set(variables) != set(names):
        raise ValueError('Result variables differ from the registered projection')
    rows = result['results']['bindings']
    for row in rows:
        if not isinstance(row, dict) or not set(row) <= set(names):
            raise ValueError('Result contains an undeclared binding')
        if any(name not in row for name, kind in columns if kind == 'count'):
            raise ValueError('A computed COUNT binding is missing')
    return rows


def term_bag(rows, columns, normalize_counts=False):
    def row_key(row):
        values = []
        for name, kind in columns:
            term = row.get(name)
            if normalize_counts and kind == 'count':
                term = {'type': 'literal', 'datatype': XSD + 'integer', 'value': str(count_value(term))}
            values.append((name, exact_term(term)))
        return tuple(values)
    return Counter(row_key(row) for row in rows)


def compare(actual, reference, columns):
    """Return separate exact-term and integer-value bags; no numeric tolerance."""
    columns = projection(columns)
    expected_rows = checked_rows(reference, columns)
    for row in expected_rows:
        for name, kind in columns:
            if kind == 'count':
                if row[name].get('datatype') != XSD + 'integer':
                    raise ValueError('The independent COUNT reference must use xsd:integer')
                count_value(row[name])
    actual_rows = checked_rows(actual, columns)
    result = {'primary_pass': term_bag(actual_rows, columns) == term_bag(expected_rows, columns),
              'count_compatible_pass': False, 'rows': len(actual_rows)}
    try:
        result['count_compatible_pass'] = (term_bag(actual_rows, columns, True)
                                            == term_bag(expected_rows, columns, True))
    except ValueError as exc:
        result['count_compatible_error'] = str(exc)
    return result
