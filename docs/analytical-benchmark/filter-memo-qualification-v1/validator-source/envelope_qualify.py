"""Count/magnitude-derived numeric checking, enabled only by an explicit freeze.

The original qualifier is unchanged. Input group identities/cardinalities must
already agree with independent Jena diagnostics; every frozen file is checked
before an engine result can be examined under this policy.
"""
from collections import Counter
from fractions import Fraction
import hashlib
import json
from pathlib import Path

from float_envelope import InputStats,mean_envelope,divide_envelope,decimal_to_binary32
from qualify import exact_term,qualify,as_float,ulps,FLOAT_COLUMNS,COUNT_COLUMNS

XSD='http://www.w3.org/2001/XMLSchema#'


def read_stats(record):
    return InputStats(record['n'],Fraction(record['exact_sum']),Fraction(record['exact_sum_magnitudes']))


def anchor(number,record):
    if number==4:
        a,b=read_stats(record['with']),read_stats(record['without'])
        bounds=divide_envelope(mean_envelope(a),mean_envelope(b))
        ideal=(a.total/a.n)/(b.total/b.n)
    else:
        stats=read_stats(record);bounds=mean_envelope(stats);ideal=stats.total/stats.n
    return bounds,ideal


def qualify_enveloped(number,actual,reference,inputs):
    if number not in (4,5,6):return qualify(number,actual,reference)
    variables=reference['head']['vars']
    if len(set(variables))!=len(variables) or len(actual['head']['vars'])!=len(variables) or set(variables)!=set(actual['head']['vars']):
        raise ValueError('Projected variables differ')
    floats=FLOAT_COLUMNS[number];counts=COUNT_COLUMNS.get(number,set())
    def key(row):
        result=[]
        for variable in variables:
            if variable in floats:continue
            term=row.get(variable)
            if variable in counts and term is not None:
                if term.get('datatype')!=XSD+'integer' or term['type'] not in ('literal','typed-literal'):
                    raise ValueError('Count datatype differs')
                result.append(('integer',int(term['value'])))
            else:result.append(exact_term(term))
        return tuple(result)
    a=actual['results']['bindings'];b=reference['results']['bindings']
    if Counter(key(row) for row in a)!=Counter(key(row) for row in b):
        raise ValueError('Entity/count result bags differ')
    entity_variable={4:'feature',5:'product',6:'reviewer'}[number]
    details={};worst_ulps=0
    for role,rows in [('reference',b),('candidate',a)]:
        for row in rows:
            entity=row[entity_variable]['value']
            if entity not in inputs['groups']:raise ValueError('Missing independently verified input group: '+entity)
            bounds,ideal=anchor(number,inputs['groups'][entity])
            for variable in floats:
                term=row.get(variable)
                if term is None:raise ValueError('Valid nonempty numeric input produced an unbound output')
                as_float(term)  # Datatype and numeric lexical validation remain mandatory.
                value=decimal_to_binary32(term['value'])
                if not bounds.contains(value):raise ValueError(role+' output is outside the exact-anchor envelope')
                item=details.setdefault(entity,{'envelope':bounds.record(),'exact_ideal':str(ideal)})
                item[role]={'value':str(value),'absolute_error':str(abs(value-ideal))}
    references={key(row):row for row in b}
    for row in a:
        ref=references[key(row)]
        for variable in floats:
            worst_ulps=max(worst_ulps,ulps(as_float(row[variable]),as_float(ref[variable])))
    def exact(rows):return Counter(tuple(exact_term(row.get(v)) for v in variables) for row in rows)
    return {'all_columns_compatible':True,'numeric_policy':'exact-anchor-binary32-v1',
            'reference_and_candidate_inside_envelopes':True,'exact_term_bag':exact(a)==exact(b),
            'max_binary32_ulps_vs_reference':worst_ulps,'rows':len(a),'numeric_details':details}


class FrozenInputs:
    def __init__(self,directory,freeze_sha256,dataset_sha256,workload_sha256):
        self.directory=Path(directory);path=self.directory/'freeze.json';raw=path.read_bytes()
        if hashlib.sha256(raw).hexdigest()!=freeze_sha256:raise ValueError('Numeric freeze identity differs')
        frozen=json.loads(raw)
        if frozen.get('state')!='frozen':raise ValueError('Numeric validator is not frozen')
        required={'envelope_qualify.py','float_envelope.py','qualify.py','workload_check.py','numeric_inputs.py','bi_sql.py'}
        if not required<=set(frozen['validator_sources']):raise ValueError('Incomplete validator source freeze')
        if not {'plan.json','count-verification.json'}<=set(frozen['input_files']):raise ValueError('Incomplete input freeze')
        self.frozen_files=set(frozen['input_files'])
        for name,digest in frozen['validator_sources'].items():
            if Path(name).name!=name:raise ValueError('Invalid validator source filename')
            if hashlib.sha256((Path(__file__).parent/name).read_bytes()).hexdigest()!=digest:
                raise ValueError('Validator source differs from freeze: '+name)
        for name,digest in frozen['input_files'].items():
            path=(self.directory/name).resolve()
            if not path.is_relative_to(self.directory.resolve()):raise ValueError('Invalid diagnostic path')
            if hashlib.sha256(path.read_bytes()).hexdigest()!=digest:raise ValueError('Input diagnostic differs from freeze: '+name)
        self.plan=json.loads((self.directory/'plan.json').read_text())
        verified=json.loads((self.directory/'count-verification.json').read_text())
        if not verified['all_counts_agree']:raise ValueError('Independent input counts are not verified')
        if verified['plan_sha256']!=hashlib.sha256((self.directory/'plan.json').read_bytes()).hexdigest():
            raise ValueError('Count verification belongs to a different plan')
        if self.plan['dataset_sha256']!=dataset_sha256 or self.plan['workload_manifest_sha256']!=workload_sha256:
            raise ValueError('Numeric inputs belong to another dataset/workload')
        self.freeze_sha256=freeze_sha256

    def for_query(self,record):
        case=self.plan['instances'].get(record['file'])
        if case is None or case['query_sha256']!=record['sha256']:raise ValueError('Query differs from frozen numeric inputs')
        return json.loads((self.directory/'sql'/(case['case']+'.json')).read_text())

    def for_instance(self,query):
        item=self.plan['instances'].get(query['file'])
        if item is None or item['query_sha256']!=query['sha256']:raise ValueError('Query does not match frozen numeric inputs')
        name='sql/'+item['case']+'.json'
        if name not in self.frozen_files:raise ValueError('Input group file was not frozen')
        return json.loads((self.directory/name).read_text())
