"""Independent examples and boundaries for SPARQL 1.1 sections 17.4.2-5."""
import json,sys,urllib.request,urllib.parse
from pathlib import Path
stage=Path(__file__).resolve().parent;sys.path.insert(0,str(stage/'bench-source'))
from qualify import exact_term
X='http://www.w3.org/2001/XMLSchema#'
def literal(value,lang=None,datatype=None):
    r={'type':'literal','value':str(value)}
    if lang:r['xml:lang']=lang
    if datatype:r['datatype']=X+datatype
    return r
cases=[]
def case(name,expr,expected):cases.append({'name':name,'expression':expr,'expected':expected})
def error(name,expr):
    case(name,expr,None)
    case(name+'-coalesce','COALESCE('+expr+',"fallback")',literal('fallback'))
for name,expr,expected in [
 ('length-unicode','STRLEN("aβ")',literal(2,datatype='integer')),
 ('upper-tag','UCASE("foo"@en)',literal('FOO','en')),
 ('lower-tag','LCASE("BAR"@en)',literal('bar','en')),
 ('substring-unicode','SUBSTR("αβγ"@el,2,1)',literal('β','el')),
 ('replace-tag','REPLACE("Abc"@en,"A","z")',literal('zbc','en')),
 ('regex-tag','REGEX("abc"@en,"^a")',literal('true',datatype='boolean')),
 ('encode-uri','ENCODE_FOR_URI("a b")',literal('a%20b')),
 ('str-number','STR(7)',literal('7')),
 ('cast-string','xsd:string(7)',literal('7')),
 ('lang-matches','LANGMATCHES(LANG("abc"@en-US),"en")',literal('true',datatype='boolean')),
 ('strlang-case','STRLANG("abc","EN")',literal('abc','en')),
 ('strdt-lexical','STRDT("01",xsd:integer)',literal('01',datatype='integer')),
 ('before-no-match','STRBEFORE("abc"@en,"z")',literal('')),
 ('after-no-match','STRAFTER("abc"@en,"z")',literal('')),
 ('before-empty-search','STRBEFORE("abc"@en,"")',literal('','en')),
 ('after-empty-search','STRAFTER("abc"@en,"")',literal('abc','en')),
 ('before-unicode','STRBEFORE("αβγ"@el,"β")',literal('α','el')),
 ('after-unicode','STRAFTER("αβγ"@el,"β")',literal('γ','el')),
 ('concat-tag','CONCAT("a"@en,"b"@en)',literal('ab','en')),
 ('concat-tag-case','CONCAT("a"@EN,"b"@en)',literal('ab','en')),
 ('concat-different-tag','CONCAT("a"@en,"b"@fr)',literal('ab')),
 ('concat-mixed-plain','CONCAT("a"@en,"b")',literal('ab')),
 ('concat-mismatch-stays-plain','CONCAT("a"@en,"b"@fr,"c"@fr)',literal('abc')),
]:case(name,expr,expected)
for fn in ['CONTAINS','STRSTARTS','STRENDS','STRBEFORE','STRAFTER']:
    for suffix,a,b,lang in [('plain','"abc"','"a"',None),('tag-plain','"abc"@en','"a"','en'),
                            ('same-tag','"abc"@en','"a"@en','en'),('tag-case','"abc"@EN','"a"@en','en')]:
        expected=literal('' if fn=='STRBEFORE' else 'bc',lang) if fn in ['STRBEFORE','STRAFTER'] else literal('false' if fn=='STRENDS' else 'true',datatype='boolean')
        case(fn+'-'+suffix,f'{fn}({a},{b})',expected)
    error(fn+'-plain-tag',f'{fn}("abc","a"@en)')
    error(fn+'-different-tag',f'{fn}("abc"@en,"a"@fr)')
for i,expr in enumerate(['STRLEN(7)','SUBSTR(7,1)','UCASE(7)','LCASE(7)','CONTAINS(7,"x")',
 'STRSTARTS(7,"x")','STRENDS(7,"x")','STRBEFORE(7,"x")','STRAFTER(7,"x")','CONCAT("x",7)',
 'REGEX(7,"x")','REPLACE(7,"x","y")','ENCODE_FOR_URI(7)','STRLEN(<urn:iri>)',
 'STRLEN("abc"^^<urn:custom-type>)','STRDT("a"@en,xsd:string)','STRDT(7,xsd:string)',
 'STRDT("a","urn:datatype")','STRLANG("a"@en,"en")','STRLANG(7,"en")','STRLANG("a","en"@en)',
 'STRLANG("a",7)']):error('type-error-'+str(i),expr)
date='"2011-01-10T14:45:13.815-05:00"^^xsd:dateTime'
for fn,value in [('YEAR',2011),('MONTH',1),('DAY',10),('HOURS',14),('MINUTES',45)]:
    case(fn+'-datetime',f'{fn}({date})',literal(value,datatype='integer'))
    error(fn+'-plain-string',f'{fn}("2011-01-10T14:45:13.815-05:00")')
out={'source':'https://www.w3.org/TR/sparql11-query/#func-strings','cases':{},'complete':False}
for c in cases:
    query='PREFIX xsd: <'+X+'> SELECT ('+c['expression']+' AS ?v) WHERE {}'
    request=urllib.request.Request('http://127.0.0.1:31439/sparql',data=urllib.parse.urlencode({'query':query}).encode(),headers={'Accept':'application/sparql-results+json'})
    with urllib.request.urlopen(request,timeout=10) as response:raw=response.read();status=response.status
    result=json.loads(raw);rows=result['results']['bindings']
    correct=(status==200 and result['head']['vars']==['v'] and len(rows)==1 and
      exact_term(rows[0].get('v'))==exact_term(c['expected']))
    (stage/'fixtures').mkdir(exist_ok=True)
    (stage/'fixtures'/(c['name']+'.rq')).write_text(query)
    (stage/'fixtures'/(c['name']+'.body')).write_bytes(raw)
    out['cases'][c['name']]={**c,'pass':correct,'http_status':status}
    (stage/'fixture-results.json').write_text(json.dumps(out,indent=2))
out['complete']=True;out['all_pass']=all(v['pass'] for v in out['cases'].values())
(stage/'fixture-results.json').write_text(json.dumps(out,indent=2))
assert out['all_pass'],[(n,c) for n,c in out['cases'].items() if not c['pass']]
