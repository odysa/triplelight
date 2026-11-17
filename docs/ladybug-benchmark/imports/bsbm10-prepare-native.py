#!/usr/bin/env python3
"""Lossless RDF-term graph mapping for native Ladybug benchmarks.

Not a general SPARQL engine. Unsupported query syntax fails closed.
"""
import argparse
import csv
import hashlib
import json
import math
import os
from pathlib import Path
import re
import subprocess
import time

XSD = 'http://www.w3.org/2001/XMLSchema#'
RDF = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#'
IRI = r'<[^<>\s]*>'
LIT = r'"(?:[^"\\]|\\.)*"(?:@[a-zA-Z0-9-]+|\^\^<[^>]*>)?'
TERM = rf'(?:{IRI}|_:[A-Za-z0-9_]+|{LIT})'
LINE = re.compile(rf'^\s*({IRI}|_:[A-Za-z0-9_]+)\s+({IRI})\s+({TERM})\s*\.\s*(?:#.*)?$')


def unescape(s):
    escapes = {'t':'\t','b':'\b','n':'\n','r':'\r','f':'\f','"':'"',"'":"'",'\\':'\\'}
    def sub(m):
        v = m[1]
        return chr(int(v[1:],16)) if v[0] in 'uU' else escapes[v]
    return re.sub(r'\\(u[0-9a-fA-F]{4}|U[0-9a-fA-F]{8}|[tbnrf"\x27\\])', sub, s)


def term(token):
    if token.startswith('<'):
        return {'type':'uri','value':unescape(token[1:-1])}
    if token.startswith('_:'):
        return {'type':'bnode','value':token[2:]}
    m = re.fullmatch(r'"((?:[^"\\]|\\.)*)"(.*)',token)
    if not m:
        raise ValueError('Unsupported RDF token: '+token)
    out = {'type':'literal','value':unescape(m[1])}
    if m[2].startswith('@'):
        out['xml:lang'] = m[2][1:].lower()
    elif m[2]:
        dt = m[2][3:-1]
        if dt != XSD+'string': out['datatype'] = dt
    return out


def key(t):
    return json.dumps(t,sort_keys=True,separators=(',',':'),ensure_ascii=False)


def prepare(nt, out):
    """Produce typed node/relationship CSVs; external sort deduplicates triples."""
    out.mkdir(parents=True,exist_ok=False)
    handles = {}
    writers = {}
    terms = {}
    resources = {}
    predicates = {}
    counts = {'source_lines':0,'unique_triples':0,'resource_nodes':0,'literal_nodes':0}
    start = time.perf_counter()
    digest = hashlib.sha256()
    for table in ('Resource','Literal'):
        f = (out/(table+'.csv')).open('w',newline='')
        handles[table] = f
        writers[table] = csv.writer(f)
    def intern(t):
        k = key(t)
        if k in terms: return terms[k]
        i = len(terms)
        kind = 'Literal' if t['type']=='literal' else 'Resource'
        terms[k] = (i,kind)
        if kind=='Resource':
            resources[k] = i
            writers[kind].writerow([i,k,t['value']])
            counts['resource_nodes'] += 1
        else:
            num = ''
            if t.get('datatype','').removeprefix(XSD) in ('integer','int','long','short','byte','decimal','double','float','nonNegativeInteger','positiveInteger'):
                num = float(t['value'])
                if not math.isfinite(num):raise ValueError('Non-finite numeric literal not supported by this mapping')
                if t.get('datatype','').removeprefix(XSD) not in ('decimal','double','float') and abs(int(t['value']))>2**53:
                    raise ValueError('Integer exceeds exact DOUBLE projection range')
            if t.get('datatype')==XSD+'dateTime' and not re.fullmatch(r'\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}',t['value']):
                raise ValueError('Date comparison mapping requires fixed timezone-free ISO timestamps')
            writers[kind].writerow([i,k,t['value'],t.get('datatype',''),t.get('xml:lang',''),num])
            counts['literal_nodes'] += 1
        return i,kind
    try:
        with nt.open('rb') as f:
            for raw in f:
                digest.update(raw)
                line = raw.decode().strip()
                if not line or line.startswith('#'): continue
                m = LINE.fullmatch(line)
                if not m: raise ValueError(f'Unsupported N-Triples line {counts["source_lines"]+1}: {line[:100]}')
                counts['source_lines'] += 1
                s,p,o = (term(x) for x in m.groups())
                si,sk = intern(s); oi,ok = intern(o)
                predicate = p['value']
                if predicate not in predicates: predicates[predicate] = f'P{len(predicates)}'
                table = predicates[predicate] + ('R' if ok=='Resource' else 'L')
                if table not in handles:
                    handles[table] = (out/(table+'.raw')).open('w')
                handles[table].write(f'{si},{oi}\n')
                if counts['source_lines']%1000000==0: print(json.dumps(counts),flush=True)
    finally:
        for f in handles.values(): f.close()
    del terms
    (out/'resource-ids.json').write_text(json.dumps(resources,separators=(',',':')))
    del resources
    rels = []
    for p in out.glob('P*.raw'):
        target = p.with_suffix('.csv')
        with target.open('wb') as dest:
            subprocess.run(['sort','-u','-S','256M','-T',str(out),str(p)],stdout=dest,check=True,env={**os.environ,'LC_ALL':'C'})
        with target.open('rb') as stream:count = sum(1 for _ in stream)
        counts['unique_triples'] += count
        rels.append({'table':p.stem,'target':'Resource' if p.stem.endswith('R') else 'Literal','rows':count})
        p.unlink()
    manifest = {'mapping':'rdf-terms-per-predicate-v1','input':str(nt.resolve()),'input_sha256':digest.hexdigest(),
                'counts':counts,'predicates':predicates,'relationships':rels,'prepare_seconds':time.perf_counter()-start}
    (out/'manifest.json').write_text(json.dumps(manifest,indent=2))
    print(json.dumps(manifest),flush=True)


def load(csvdir, dbpath, memory_mb, threads):
    import ladybug as lb
    # The Python module's import name is verified against the pinned wheel at smoke time.
    manifest = json.loads((csvdir/'manifest.json').read_text())
    start = time.perf_counter()
    db = lb.Database(str(dbpath),buffer_pool_size=memory_mb*1024**2,max_num_threads=threads)
    conn = lb.Connection(db)
    conn.execute('CREATE NODE TABLE Resource(id INT64, term STRING, value STRING, PRIMARY KEY(id))')
    conn.execute('CREATE NODE TABLE Literal(id INT64, term STRING, value STRING, datatype STRING, lang STRING, num DOUBLE, PRIMARY KEY(id))')
    def copy(table):
        conn.execute(f'COPY {table} FROM '+json.dumps(str((csvdir/(table+'.csv')).resolve()))+' (HEADER=false)')
    for table in ('Resource','Literal'): copy(table)
    for rel in manifest['relationships']:
        conn.execute(f'CREATE REL TABLE {rel["table"]}(FROM Resource TO {rel["target"]})')
        copy(rel['table'])
    observed = {}
    for table in ('Resource','Literal'):
        observed[table] = conn.execute(f'MATCH (n:{table}) RETURN count(*)').get_next()[0]
    observed['relationships'] = sum(conn.execute(f'MATCH ()-[r:{rel["table"]}]->() RETURN count(*)').get_next()[0] for rel in manifest['relationships'])
    assert observed == {'Resource':manifest['counts']['resource_nodes'],'Literal':manifest['counts']['literal_nodes'],'relationships':manifest['counts']['unique_triples']},observed
    conn.execute('CHECKPOINT')
    record = {'load_seconds':time.perf_counter()-start,'observed':observed,'memory_mb':memory_mb,'threads':threads}
    (csvdir/'load.json').write_text(json.dumps(record,indent=2)); print(json.dumps(record),flush=True)
    conn.close(); db.close()


def compile_bgp(query, manifest, resources):
    prefixes = dict(re.findall(r'PREFIX\s+([\w-]+):\s*<([^>]+)>',query,re.I))
    query = re.sub(r'PREFIX\s+[\w-]+:\s*<[^>]+>','',query,flags=re.I).strip()
    m = re.fullmatch(r'SELECT\s+(.*?)\s+WHERE\s*\{(.*)\}\s*([^{}]*)',query,re.I|re.S)
    if not m: raise ValueError('Unsupported SELECT syntax')
    projection = m[1].split()
    distinct = projection[0].upper()=='DISTINCT'
    if distinct:projection=projection[1:]
    if not all(re.fullmatch(r'\?\w+',v) for v in projection): raise ValueError('Unsupported projection')
    tok = re.compile(rf'{LIT}|{IRI}|\?\w+|[\w-]+:[\w-]+|\ba\b')
    def balanced(s,pos,left,right):
        assert s[pos]==left
        start=pos+1;depth=1;pos+=1
        while pos<len(s):
            quoted=re.match(r'"(?:[^"\\]|\\.)*"|<[^<>\s]*>',s[pos:])
            if quoted:pos+=quoted.end();continue
            if s[pos]==left:depth+=1
            if s[pos]==right:
                depth-=1
                if not depth:return s[start:pos],pos+1
            pos+=1
        raise ValueError('Unclosed group/expression')
    def parse(body):
        triples=[];opts=[];filters=[];pos=0
        while pos<len(body):
            if body[pos].isspace() or body[pos]=='.':pos+=1;continue
            kw=re.match(r'(OPTIONAL|FILTER)\s*',body[pos:],re.I)
            if kw:
                pos+=kw.end();optional=kw[1].upper()=='OPTIONAL'
                if not optional and body[pos]!='(':
                    fn=re.match(r'(\w+)\s*',body[pos:])
                    if not fn:raise ValueError('Unsupported FILTER expression')
                    pos+=fn.end()
                    content,pos=balanced(body,pos,'(',')')
                    filters.append(fn[1]+'('+content+')');continue
                content,pos=balanced(body,pos,'{' if optional else '(','}' if optional else ')')
                (opts if optional else filters).append(parse(content) if optional else content)
                continue
            triple=[]
            for _ in range(3):
                while pos<len(body) and body[pos].isspace():pos+=1
                t=tok.match(body,pos)
                if not t:raise ValueError('Unsupported syntax near '+body[pos:pos+60])
                triple.append(t[0]);pos=t.end()
            triples.append(triple)
        return triples,opts,filters
    group=parse(m[2])
    def expand(t):
        if t=='a':return RDF+'type'
        if t.startswith('<'):return term(t)['value']
        a,b=t.split(':',1);return prefixes[a]+b
    constants={};bound=set()
    def node(t):
        if t.startswith('?'):
            bound.add(t);return 'v_'+t[1:]
        iri=expand(t)
        if iri not in constants:
            alias='c_'+str(len(constants));constants[iri]=alias
            i=resources.get(key({'type':'uri','value':iri}),-1)
            constants[iri]=(alias,i)
        return constants[iri][0]
    rels={r['table'] for r in manifest['relationships']}
    def expression(s):
        s=re.sub(r'langMatches\s*\(\s*lang\s*\(\s*(\?\w+)\s*\)\s*,\s*"EN"\s*\)',lambda m:f'(v_{m[1][1:]}.lang = "en" OR starts_with(v_{m[1][1:]}.lang,"en-"))',s,flags=re.I)
        s=re.sub(r'!\s*bound\s*\(\s*(\?\w+)\s*\)',lambda m:f'(v_{m[1][1:]} IS NULL)',s,flags=re.I)
        # URI != resource and numeric arithmetic are separate typed operations.
        def iri_compare(m):
            ident=resources.get(key(term(m[1])),-1)
            return f'({ident} {m[2]} v_{m[3][1:]}.id)'
        s=re.sub(r'(<[^>]+>)\s*(!=|=)\s*(\?\w+)',iri_compare,s)
        # The benchmark's date constants/values use one timezone-free ISO format.
        s=re.sub(r'(\?\w+)\s*(>|<|>=|<=)\s*("[^"]+"\^\^<[^>]+>)',lambda m:f'v_{m[1][1:]}.value {m[2]} '+json.dumps(term(m[3])['value']),s)
        s=re.sub(r'\?(\w+)',r'v_\1.num',s)
        s=s.replace('&&',' AND ').replace('||',' OR ').replace('!=','<>')
        # Reject untranslated functions, IRIs and unsupported operators.
        if '?' in s or '^^' in s or re.search(r'\b(?:regex|str|bound|lang)\s*\(',s,re.I):raise ValueError('Unsupported FILTER '+s)
        return s
    def render(g,optional=False):
        triples,opts,filters=g;patterns=[];constraints=[]
        mandatory_vars={t for triple in triples for t in triple if t.startswith('?')}
        before_optional=[];after_optional=[]
        for f in filters:
            (before_optional if set(re.findall(r'\?\w+',f))<=mandatory_vars else after_optional).append(f)
        if optional and after_optional:raise ValueError('Filter over nested optional bindings is not translated')
        usedconstants=set()
        for s,p,o in triples:
            a=node(s);b=node(o);pred=expand(p)
            for t in (s,o):
                if not t.startswith('?'):usedconstants.add(expand(t))
            if pred not in manifest['predicates']:raise ValueError('Predicate missing from dataset: '+pred)
            base=manifest['predicates'][pred]
            labels=[base+suffix for suffix in ('R','L') if base+suffix in rels]
            patterns.append(f'({a})-[:'+ '|'.join(labels)+f']->({b})')
        constraints=[f'{constants[c][0]}.id = {constants[c][1]}' for c in sorted(usedconstants)]
        text=('OPTIONAL MATCH ' if optional else 'MATCH ')+', '.join(patterns)
        constraints += [expression(s) for s in before_optional]
        if constraints:text+=' WHERE '+' AND '.join('('+s+')' for s in constraints)
        for child in opts:text+=' '+render(child,True)
        if after_optional:text+=' WITH * WHERE '+' AND '.join('('+expression(s)+')' for s in after_optional)
        return text
    cypher=render(group)
    if not set(projection)<=bound:raise ValueError('Unbound projection')
    tail=m[3].strip();order=[]
    ordermatch=re.search(r'ORDER\s+BY\s+(.*?)(?=\bOFFSET\b|\bLIMIT\b|$)',tail,re.I)
    if ordermatch:
        expr=ordermatch[1].strip();desc=False
        dm=re.fullmatch(r'DESC\s*\(\s*(\?\w+)\s*\)',expr,re.I)
        cast=re.fullmatch(r'xsd:double\s*\(\s*str\s*\(\s*(\?\w+)\s*\)\s*\)',expr,re.I)
        if dm:expr=dm[1];desc=True
        elif cast:expr=cast[1]
        if not re.fullmatch(r'\?\w+',expr):raise ValueError('Unsupported ORDER BY')
        order=[('v_'+expr[1:]+('.num' if cast else '.value'),desc)]
        tail=tail[:ordermatch.start()]+tail[ordermatch.end():]
    limit=re.search(r'LIMIT\s+(\d+)',tail,re.I);offset=re.search(r'OFFSET\s+(\d+)',tail,re.I)
    extra=re.sub(r'(?:LIMIT|OFFSET)\s+\d+','',tail,flags=re.I).strip()
    if extra:raise ValueError('Unsupported query modifier: '+extra)
    ret=['v_'+v[1:]+'.term AS '+v[1:] for v in projection]
    ret += [expr+f' AS sort_{i}' for i,(expr,_) in enumerate(order)]
    cypher+=' RETURN '+('DISTINCT ' if distinct else '')+', '.join(ret)
    if order:cypher+=' ORDER BY '+', '.join(f'sort_{i}'+(' DESC' if desc else '') for i,(_,desc) in enumerate(order))
    if offset:cypher+=' SKIP '+offset[1]
    if limit:cypher+=' LIMIT '+limit[1]
    return {'vars':[v[1:] for v in projection],'cypher':cypher}


def compile_queries(csvdir,qdir,out):
    manifest=json.loads((csvdir/'manifest.json').read_text())
    resources=json.loads((csvdir/'resource-ids.json').read_text())
    result={}
    for q in qdir.glob('*.rq'):
        source=q.read_text()
        try:
            if re.search(r'\bUNION\b',source,re.I):
                from union_queries import compile_union
                rec=compile_union(source,manifest,resources,compile_bgp,key)
            else:rec=compile_bgp(source,manifest,resources)
        except ValueError as e: rec={'unsupported':str(e)}
        result[q.stem]={'source_sha256':hashlib.sha256(source.encode()).hexdigest(),**rec}
    out.write_text(json.dumps(result,indent=2))


def serve(dbpath, queries, port, memory_mb, threads):
    import ladybug as lb
    import http.server
    import urllib.parse
    compiled=json.loads(queries.read_text())
    byhash={q['source_sha256']:q for q in compiled.values()}
    db=lb.Database(str(dbpath),buffer_pool_size=memory_mb*1024**2,max_num_threads=threads,read_only=True)
    conn=lb.Connection(db)
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            try:
                args=urllib.parse.parse_qs(self.rfile.read(int(self.headers['Content-Length'])).decode())
                q=args['query'][0]
                rec=byhash[hashlib.sha256(q.encode()).hexdigest()]
                if 'unsupported' in rec:raise ValueError(rec['unsupported'])
                result=conn.execute(rec['cypher'])
                bindings=[]
                while result.has_next():
                    row=result.get_next()
                    bindings.append({v:({'type':'uri','value':rec['predicate_labels'][x]} if v in rec.get('predicate_columns',[]) else json.loads(x)) for v,x in zip(rec['vars'],row) if x is not None})
                body=json.dumps({'head':{'vars':rec['vars']},'results':{'bindings':bindings}},ensure_ascii=False).encode()
                self.send_response(200);self.send_header('Content-Type','application/sparql-results+json');self.send_header('Content-Length',str(len(body)))
                self.send_header('X-Ladybug-Execution-Ms',str(result.get_execution_time()))
                self.send_header('X-Ladybug-Compilation-Ms',str(result.get_compiling_time()))
                self.end_headers();self.wfile.write(body)
            except Exception as e:
                body=str(e).encode();self.send_response(500);self.end_headers();self.wfile.write(body)
        def log_message(self,*args):pass
    print('READY',flush=True)
    try:http.server.HTTPServer(('127.0.0.1',port),Handler).serve_forever()
    finally:conn.close();db.close()


if __name__=='__main__':
    p=argparse.ArgumentParser();sub=p.add_subparsers(dest='mode',required=True)
    s=sub.add_parser('prepare');s.add_argument('nt',type=Path);s.add_argument('out',type=Path)
    s=sub.add_parser('load');s.add_argument('csvdir',type=Path);s.add_argument('dbpath',type=Path)
    s=sub.add_parser('compile');s.add_argument('csvdir',type=Path);s.add_argument('qdir',type=Path);s.add_argument('out',type=Path)
    s=sub.add_parser('serve');s.add_argument('dbpath',type=Path);s.add_argument('queries',type=Path);s.add_argument('--port',type=int,default=7981)
    for s in [sub.choices['load'],sub.choices['serve']]:
        s.add_argument('--memory-mb',type=int,default=10240);s.add_argument('--threads',type=int,default=4)
    args=vars(p.parse_args());mode=args.pop('mode');globals()[{'compile':'compile_queries'}.get(mode,mode)](**args)
