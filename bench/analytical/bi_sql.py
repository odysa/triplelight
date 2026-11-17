"""Hand translations of the eight pinned BSBM BI templates, not a SPARQL engine.

Physical baseline: one triples(s VARCHAR, p VARCHAR, o STRUCT(kind UTINYINT,
value VARCHAR, datatype VARCHAR, lang VARCHAR)) table, deduplicated as an RDF
graph. BSBM subjects/resources are named IRIs; import rejects blank subjects.
No property tables, precomputed joins or aggregate materializations are used.
All numeric input casts are binary32, as specified by these BI queries.
"""
BSBM = 'http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/'
RDF_TYPE = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type'
REV = 'http://purl.org/stuff/rev#'
DC = 'http://purl.org/dc/elements/1.1/'


def literal(value):
    return "'" + value.replace("'", "''") + "'"


def iri(value):
    if not value.startswith('<') or not value.endswith('>'):
        raise ValueError('Expected an exported IRI parameter')
    return literal(value[1:-1])


def resources(predicate):
    return f"SELECT s, o.value AS obj FROM triples WHERE p={literal(predicate)} AND o.kind=0"


def literals(predicate):
    return f"SELECT s, o.value AS lex, o.datatype AS datatype, o.lang AS lang FROM triples WHERE p={literal(predicate)} AND o.kind=2"


def mean32(expression):
    # An invalid aggregate input must not silently become SQL AVG's ignored NULL.
    return f"CASE WHEN COUNT({expression})=COUNT(*) THEN CAST(AVG({expression}) AS REAL) END"


def common():
    definitions = {'types':resources(RDF_TYPE)}
    for name in ('producer','country','reviewFor','productFeature','product','vendor'):
        definitions[name] = resources(BSBM+name)
    definitions['reviewer'] = resources(REV+'reviewer')
    definitions['price'] = literals(BSBM+'price')
    definitions['dates'] = literals(DC+'date')
    definitions['ratings'] = ' UNION ALL '.join(
        f"SELECT s AS review, TRY_CAST(o.value AS REAL) AS score FROM triples WHERE p={literal(BSBM+'rating'+str(i))} AND o.kind=2"
        for i in range(1,5))
    return definitions


OUTPUT = {
    1:[('productType','iri'),('reviewCount','integer')],
    2:[('otherProduct','iri'),('sameFeatures','integer')],
    3:[('product','iri'),('ratio','float')],
    4:[('feature','iri'),('priceRatio','float')],
    5:[('country','iri'),('product','iri'),('nrOfReviews','integer'),('avgPrice','float')],
    6:[('reviewer','iri'),('reviewerAvgScore','float')],
    7:[('product','iri')],
    8:[('vendor','iri'),('cheapExpensiveRatio','float')],
}


def translate(number, parameters, q7_diagnostic=False):
    definitions = common()
    p = parameters
    if number == 1:
        sql = f"""SELECT ty.s AS productType, count(*) AS reviewCount
FROM types AS ty JOIN types AS pt ON pt.obj=ty.s
JOIN producer pr ON pr.s=pt.s JOIN country pc ON pc.s=pr.obj
JOIN reviewFor rf ON rf.obj=pt.s JOIN reviewer rr ON rr.s=rf.s
JOIN country rc ON rc.s=rr.obj
WHERE ty.obj={literal(BSBM+'ProductType')} AND pc.obj={iri(p['Country1'])} AND rc.obj={iri(p['Country2'])}
GROUP BY ty.s ORDER BY reviewCount DESC, productType LIMIT 10"""
    elif number == 2:
        definitions['shared'] = f"""SELECT b.s AS otherProduct, count(*) AS sameFeatures
FROM productFeature a JOIN productFeature b ON a.obj=b.obj
WHERE a.s={iri(p['Product'])} GROUP BY b.s"""
        sql = f"""SELECT a.otherProduct, a.sameFeatures FROM shared a JOIN types t ON t.s=a.otherProduct
WHERE t.obj={literal(BSBM+'Product')} AND a.otherProduct<>{iri(p['Product'])}
ORDER BY sameFeatures DESC, otherProduct LIMIT 10"""
    elif number == 3:
        for label, start, end in [('now','ConsecutiveMonth_1','ConsecutiveMonth_2'),('before','ConsecutiveMonth_0','ConsecutiveMonth_1')]:
            definitions[label] = f"""SELECT rf.obj AS product, count(*) AS n
FROM reviewFor rf JOIN dates d ON d.s=rf.s
WHERE d.datatype='http://www.w3.org/2001/XMLSchema#date'
AND TRY_CAST(d.lex AS DATE)>=CAST({literal(p[start])} AS DATE)
AND TRY_CAST(d.lex AS DATE)<CAST({literal(p[end])} AS DATE)
GROUP BY rf.obj"""
        sql = """SELECT a.product, CAST(CAST(a.n AS REAL)/CAST(b.n AS REAL) AS REAL) AS ratio
FROM now a JOIN before b ON a.product=b.product WHERE b.n>0
ORDER BY ratio DESC, product LIMIT 10"""
    elif number in (4,5,7,8):
        definitions['products'] = f"SELECT s AS product FROM types WHERE obj={iri(p['ProductType'])}"
        definitions['offers'] = """SELECT products.product, prod.s AS offer, price.lex AS price
FROM products JOIN product prod ON prod.obj=products.product JOIN price ON price.s=prod.s"""
        if number == 4:
            definitions['featureSet'] = "SELECT DISTINCT f.obj AS feature FROM products p JOIN productFeature f ON f.s=p.product"
            definitions['withFeature'] = f"""SELECT f.obj AS feature, {mean32('TRY_CAST(o.price AS REAL)')} AS price
FROM offers o JOIN productFeature f ON f.s=o.product GROUP BY f.obj"""
            definitions['withoutFeature'] = f"""SELECT f.feature, {mean32('TRY_CAST(o.price AS REAL)')} AS price
FROM featureSet f CROSS JOIN offers o
WHERE NOT EXISTS (SELECT 1 FROM productFeature pf WHERE pf.s=o.product AND pf.obj=f.feature)
GROUP BY f.feature"""
            sql = """SELECT a.feature, CAST(a.price/b.price AS REAL) AS priceRatio
FROM withFeature a JOIN withoutFeature b ON a.feature=b.feature
ORDER BY priceRatio DESC NULLS LAST, feature LIMIT 10"""
        elif number == 5:
            definitions['reviewsByCountry'] = """SELECT c.obj AS country, p.product, count(*) AS n
FROM products p JOIN reviewFor rf ON rf.obj=p.product JOIN reviewer rr ON rr.s=rf.s
JOIN country c ON c.s=rr.obj GROUP BY c.obj,p.product"""
            definitions['mostReviews'] = "SELECT country, max(n) AS n FROM reviewsByCountry GROUP BY country"
            # ?country is unbound in this subquery, not a SQL equality key.
            definitions['prices'] = f"SELECT product, {mean32('TRY_CAST(price AS REAL)')} AS avgPrice FROM offers GROUP BY product"
            sql = """SELECT r.country,r.product,r.n AS nrOfReviews,p.avgPrice
FROM reviewsByCountry r JOIN mostReviews m ON r.country=m.country AND r.n=m.n
JOIN prices p ON p.product=r.product ORDER BY nrOfReviews DESC,country,product"""
        elif number == 7:
            definitions['offerCounts'] = """SELECT p.product,count(*) AS n FROM products p
JOIN product o ON o.obj=p.product GROUP BY p.product"""
            blocked = f"EXISTS (SELECT 1 FROM product o JOIN vendor v ON v.s=o.s JOIN country c ON c.s=v.obj WHERE o.obj=x.product AND c.obj={iri(p['Country'])})"
            if q7_diagnostic:
                sql = f"SELECT x.product,x.n AS offerCount,{blocked} AS blocked FROM offerCounts x"
            else:
                definitions['topProducts'] = "SELECT product FROM offerCounts ORDER BY n DESC LIMIT 1000"
                sql = f"SELECT x.product FROM topProducts x WHERE NOT {blocked}"
        else:
            definitions['withVendor'] = "SELECT o.*,v.obj AS vendor FROM offers o JOIN vendor v ON v.s=o.offer"
            definitions['productAvg'] = f"SELECT product,{mean32('TRY_CAST(price AS REAL)')} AS price FROM withVendor GROUP BY product"
            definitions['below'] = """SELECT o.vendor,count(*) AS n FROM withVendor o JOIN productAvg p ON o.product=p.product
WHERE TRY_CAST(o.price AS REAL)<p.price GROUP BY o.vendor"""
            definitions['totals'] = """SELECT v.obj AS vendor,count(*) AS n FROM products p JOIN product o ON o.obj=p.product
JOIN vendor v ON v.s=o.s GROUP BY v.obj"""
            sql = """SELECT b.vendor, CAST(CAST(b.n AS REAL)/CAST(t.n AS REAL) AS REAL) AS cheapExpensiveRatio
FROM below b JOIN totals t ON b.vendor=t.vendor
ORDER BY cheapExpensiveRatio DESC,vendor LIMIT 10"""
    elif number == 6:
        definitions['producerReviews'] = f"""SELECT rf.s AS review FROM producer p JOIN reviewFor rf ON rf.obj=p.s
WHERE p.obj={iri(p['Producer'])}"""
        definitions['globalAvg'] = f"""SELECT {mean32('r.score')} AS score
FROM producerReviews pr JOIN ratings r ON r.review=pr.review"""
        sql = f"""SELECT rr.obj AS reviewer,{mean32('r.score')} AS reviewerAvgScore
FROM producerReviews pr JOIN ratings r ON r.review=pr.review JOIN reviewer rr ON rr.s=pr.review CROSS JOIN globalAvg g
GROUP BY rr.obj HAVING {mean32('r.score')}>CAST(min(g.score)*CAST(1.5 AS REAL) AS REAL)"""
    else:
        raise ValueError('Unknown BI template')
    return 'WITH '+',\n'.join(name+' AS ('+text+')' for name,text in definitions.items())+'\n'+sql
