"""Use the native parameterized API, whose driver retains prepared plans."""
import re


def parameterize(sql):
    parameters={}
    def replace(m):
        name='rdf_id_'+str(len(parameters));parameters[name]=int(m[2])
        return m[1]+'$'+name
    sql=re.sub(r'(\.id\s*=\s*)(-?\d+)\b',replace,sql)
    if not parameters:
        if ' UNION ' in sql.upper():raise ValueError('No-constant UNION parameterization not supported')
        head,ret=sql.split(' RETURN ',1)
        if ' UNWIND ' in head:head+=' WITH * WHERE $benchmark_enabled'
        elif ' WHERE ' in head:head+=' AND $benchmark_enabled'
        else:head+=' WHERE $benchmark_enabled'
        sql=head+' RETURN '+ret;parameters['benchmark_enabled']=True
    return sql,parameters
