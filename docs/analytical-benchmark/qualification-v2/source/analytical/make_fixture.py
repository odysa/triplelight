"""Small BI graph with unbound-country, duplicate-score and scope counterexamples."""
import json
from pathlib import Path
import re

BSBM='http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/'
RDF='http://www.w3.org/1999/02/22-rdf-syntax-ns#'
REV='http://purl.org/stuff/rev#'
DC='http://purl.org/dc/elements/1.1/'
XSD='http://www.w3.org/2001/XMLSchema#'
US='http://downlode.org/rdf/iso-3166/countries#US'
DE='http://downlode.org/rdf/iso-3166/countries#DE'


def make(folder,cutoff=False,invalid_price=False,invalid_rating=False):
    folder.mkdir(parents=True,exist_ok=False)
    rows=[]
    def resource(s,p,o):rows.append(f'<{s}> <{p}> <{o}> .')
    def value(s,p,v,dt):rows.append(f'<{s}> <{p}> {json.dumps(str(v))}^^<{dt}> .')
    for t in ('Type0','Type1'):resource('urn:'+t,RDF+'type',BSBM+'ProductType')
    for product in ('p1','p2','p3'):
        for t in (BSBM+'Product','urn:Type0','urn:Type1'):resource('urn:'+product,RDF+'type',t)
    for product,features in [('p1',['f1']),('p2',['f1','f2']),('p3',['f2'])]:
        for feature in features:resource('urn:'+product,BSBM+'productFeature','urn:'+feature)
    for p,producer in [('p1','pr1'),('p2','pr1'),('p3','pr2')]:resource('urn:'+p,BSBM+'producer','urn:'+producer)
    for who,country in [('pr1',US),('pr2',DE),('ra',US),('rb',DE),('rc',US),('v1',US),('v2',DE)]:
        resource('urn:'+who,BSBM+'country',country)
    for review,product,person,date,scores in [
        ('r1','p1','ra','2007-07-05',[4,4]),('r2','p1','rb','2007-06-15',[5]),
        ('r3','p2','rc','2007-07-10',[5]),('r4','p2','ra','2007-06-15',[1]),
        ('r5','p3','ra','2007-07-06',[5]),('r6','p3','rb','2007-06-10',[3]),
        *[(f'orphan{i}','p2',None,'2007-07-08',[0]) for i in range(5)]]:
        subject='urn:'+review;resource(subject,BSBM+'reviewFor','urn:'+product)
        if person:resource(subject,REV+'reviewer','urn:'+person)
        value(subject,DC+'date',date,XSD+'date')
        for i,score in enumerate(scores,1):value(subject,BSBM+'rating'+str(i),score,XSD+'integer')
    for offer,product,vendor,price in [('o1','p1','v1',10),('o2','p1','v2',20),('o3','p2','v2',5),
                                       ('o4','p2','v1',15),('o5','p3','v2',40),('o6','p3','v2',60)]:
        resource('urn:'+offer,BSBM+'product','urn:'+product)
        resource('urn:'+offer,BSBM+'vendor','urn:'+vendor)
        value('urn:'+offer,BSBM+'price',price,BSBM+'USD')
    # Graph deduplication must not change solution multiplicities.
    if invalid_price:value('urn:o1',BSBM+'price','bad',BSBM+'USD')
    if invalid_rating:value('urn:orphan0',BSBM+'rating1','bad',XSD+'integer')
    if cutoff:
        for i in range(1002):
            product=f'urn:tie{i:04d}';offer=f'urn:tieOffer{i:04d}'
            resource(product,RDF+'type',BSBM+'Product')
            resource(product,RDF+'type','urn:Type0')
            resource(offer,BSBM+'product',product)
            resource(offer,BSBM+'vendor','urn:v1' if i%2==0 else 'urn:v2')
    rows.append(rows[-1])
    (folder/'data.nt').write_text('\n'.join(rows)+'\n')
    params={'Country1':f'<{US}>','Country2':f'<{DE}>','Country':f'<{US}>',
            'Product':'<urn:p1>','ProductType':'<urn:Type0>','Producer':'<urn:pr1>',
            'ConsecutiveMonth_0':'2007-06-01','ConsecutiveMonth_1':'2007-07-01','ConsecutiveMonth_2':'2007-08-01'}
    entries=[];templates=Path(__file__).resolve().parent/'bsbm-bi/templates'
    queries=folder/'queries';queries.mkdir()
    for n in range(1,9):
        text=(templates/f'query{n}.txt').read_text()
        text=re.sub(r'%([A-Za-z0-9_]+)%',lambda m:params[m[1]],text)
        file=f'q{n}.rq';(queries/file).write_text(text)
        entries.append({'template':n,'file':file,'parameters':params})
    if cutoff:
        diagnostic=f'''PREFIX bsbm: <{BSBM}>
SELECT ?product ?offerCount (EXISTS {{ ?offer bsbm:product ?product . ?offer bsbm:vendor ?vendor . ?vendor bsbm:country <{US}> }} AS ?blocked)
WHERE {{ {{ SELECT ?product (COUNT(?offer) AS ?offerCount) {{ ?product a <urn:Type0> . ?offer bsbm:product ?product }} GROUP BY ?product }} }}'''
        (folder/'q7-diagnostic.rq').write_text(diagnostic)
    (folder/'manifest.json').write_text(json.dumps({'fixture':True,'cutoff':cutoff,
        'invalid_price':invalid_price,'invalid_rating':invalid_rating,'queries':entries,
        'expected_q6_reviewers':{'urn:ra':3.0,'urn:rb':5.0,'urn:rc':5.0}},indent=2))


if __name__=='__main__':
    import argparse
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('folder',type=Path);p.add_argument('--cutoff',action='store_true')
    p.add_argument('--invalid-price',action='store_true');p.add_argument('--invalid-rating',action='store_true')
    args=p.parse_args();make(args.folder,args.cutoff,args.invalid_price,args.invalid_rating)
