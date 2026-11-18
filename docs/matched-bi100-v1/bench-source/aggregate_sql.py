#!/usr/bin/env python3
"""Translate only the curated flat-BGP COUNT workload to the lexical triples table.

Not a general SPARQL parser. Rejects unsupported syntax. All variables use a
complete RDF-term struct, including datatype/language, so cross-position joins
and DISTINCT preserve identity. The table requires named-IRI subjects/predicates
and set semantics, as produced by duck_load.py.
"""
from dataclasses import dataclass
import re

XSD = 'http://www.w3.org/2001/XMLSchema#'
TOKEN = re.compile(r'\s*(<[^<>\s]*>|\?[A-Za-z_][A-Za-z_0-9]*|[A-Za-z_][A-Za-z_0-9-]*:[A-Za-z_0-9.-]*|[A-Za-z_][A-Za-z_0-9-]*|[{}().*])')
VAR = re.compile(r'\?[A-Za-z_][A-Za-z_0-9]*\Z')


def literal(value):
    return "'" + value.replace("'", "''") + "'"


def ident(value):
    return '"' + value.replace('"', '""') + '"'


def iri_term(value):
    return ('struct_pack(kind := 0::UTINYINT, value := ' + literal(value)
            + ', datatype := NULL::VARCHAR, lang := NULL::VARCHAR)')


@dataclass(frozen=True)
class Translation:
    sql: str
    columns: tuple  # (name, 'term' or 'count')

    def result_json(self, rows):
        bindings = []
        for row in rows:
            if len(row) != len(self.columns):
                raise ValueError('SQL output width differs from declared projection')
            binding = {}
            for (name, kind), value in zip(self.columns, row):
                if kind == 'count':
                    if not isinstance(value, int) or value < 0:
                        raise ValueError('Expected an exact nonnegative COUNT')
                    binding[name] = {'type': 'literal', 'value': str(value), 'datatype': XSD+'integer'}
                else:
                    if value['kind'] == 0:
                        binding[name] = {'type': 'uri', 'value': value['value']}
                    elif value['kind'] == 2:
                        term = {'type': 'literal', 'value': value['value']}
                        if value['lang']:
                            term['xml:lang'] = value['lang']
                        elif value['datatype']:
                            term['datatype'] = value['datatype']
                        binding[name] = term
                    else:
                        raise ValueError('Unexpected term kind for this named-resource table')
            bindings.append(binding)
        return {'head': {'vars': [c[0] for c in self.columns]}, 'results': {'bindings': bindings}}


def translate(query):
    tokens = []
    pos = 0
    while query[pos:].strip():
        match = TOKEN.match(query, pos)
        if match is None:
            raise ValueError('Unsupported lexical input at offset '+str(pos))
        tokens.append(match[1]); pos = match.end()
    i = 0
    def take():
        nonlocal i
        if i == len(tokens):
            raise ValueError('Unexpected end of query')
        value = tokens[i]; i += 1
        return value
    def expect(value):
        if take().upper() != value:
            raise ValueError('Expected '+value)
    prefixes = {}
    while i < len(tokens) and tokens[i].upper() == 'PREFIX':
        take(); name = take(); uri = take()
        if not name.endswith(':') or not uri.startswith('<') or name[:-1] in prefixes:
            raise ValueError('Invalid or duplicate PREFIX')
        prefixes[name[:-1]] = uri[1:-1]
    def term(token):
        if VAR.fullmatch(token): return token
        if token.startswith('<'): return ('iri', token[1:-1])
        if token == 'a': return ('iri', 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type')
        if ':' in token:
            prefix, suffix = token.split(':', 1)
            if prefix in prefixes: return ('iri', prefixes[prefix]+suffix)
        raise ValueError('Only variables and named IRIs are supported in this workload')
    expect('SELECT')
    projection = []
    while i < len(tokens) and tokens[i].upper() != 'WHERE':
        token = take()
        if VAR.fullmatch(token):
            projection.append((token, 'term', token, False))
        elif token == '(':
            expect('COUNT'); expect('(')
            source = take(); distinct = source.upper() == 'DISTINCT'
            if distinct: source = take()
            if source != '*' and not VAR.fullmatch(source):
                raise ValueError('COUNT supports only * or a variable')
            if distinct and source == '*': raise ValueError('COUNT DISTINCT * unsupported')
            expect(')'); expect('AS'); alias = take(); expect(')')
            if not VAR.fullmatch(alias): raise ValueError('Expected COUNT alias')
            projection.append((alias, 'count', source, distinct))
        else: raise ValueError('Unsupported SELECT projection')
    expect('WHERE'); expect('{')
    triples = []
    while i < len(tokens) and tokens[i] != '}':
        triples.append(tuple(term(take()) for _ in range(3)))
        if i < len(tokens) and tokens[i] == '.': take()
        elif i < len(tokens) and tokens[i] != '}': raise ValueError('Expected triple separator')
    expect('}')
    group = []
    if i < len(tokens):
        expect('GROUP'); expect('BY')
        while i < len(tokens):
            token = take()
            if not VAR.fullmatch(token): raise ValueError('Only variable GROUP BY supported')
            group.append(token)
    if not triples or not projection or not any(p[1] == 'count' for p in projection):
        raise ValueError('Expected nonempty BGP and COUNT projection')
    if len({p[0] for p in projection}) != len(projection): raise ValueError('Duplicate projection variable')
    bound = {}; conditions = []
    for n, triple in enumerate(triples):
        for column, value in zip(('s', 'p', 'o'), triple):
            ref = f't{n}.{column}'
            expr = ref if column == 'o' else ('struct_pack(kind := 0::UTINYINT, value := '+ref
                    + ', datatype := NULL::VARCHAR, lang := NULL::VARCHAR)')
            if isinstance(value, str) and VAR.fullmatch(value):
                if value in bound: conditions.append(expr+' = '+bound[value])
                else: bound[value] = expr
            else:
                # Keep named subject/predicate predicates as scalar comparisons
                # so DuckDB can push them into the lexical table scan.
                conditions.append((expr+' = '+iri_term(value[1])) if column == 'o'
                                  else ref+' = '+literal(value[1]))
    if any(g not in bound for g in group) or len(set(group)) != len(group):
        raise ValueError('Invalid GROUP BY variables')
    output = []
    for alias, kind, source, distinct in projection:
        if kind == 'term':
            if source not in group: raise ValueError('Nonaggregated projection is not a group key')
            expression = bound[source]
        else:
            if source != '*' and source not in bound: raise ValueError('COUNT variable absent from BGP')
            expression = 'COUNT('+('DISTINCT ' if distinct else '')+('*' if source == '*' else bound[source])+')'
        output.append(expression+' AS '+ident(alias[1:]))
    sql = 'SELECT '+', '.join(output)+' FROM '+', '.join(f'triples t{n}' for n in range(len(triples)))
    if conditions: sql += ' WHERE '+' AND '.join(conditions)
    if group: sql += ' GROUP BY '+', '.join(bound[g] for g in group)
    return Translation(sql, tuple((p[0][1:], p[1]) for p in projection))
