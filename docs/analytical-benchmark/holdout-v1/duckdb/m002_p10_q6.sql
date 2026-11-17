WITH types AS (SELECT s, o.value AS obj FROM triples WHERE p='http://www.w3.org/1999/02/22-rdf-syntax-ns#type' AND o.kind=0),
producer AS (SELECT s, o.value AS obj FROM triples WHERE p='http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/producer' AND o.kind=0),
country AS (SELECT s, o.value AS obj FROM triples WHERE p='http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/country' AND o.kind=0),
reviewFor AS (SELECT s, o.value AS obj FROM triples WHERE p='http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/reviewFor' AND o.kind=0),
productFeature AS (SELECT s, o.value AS obj FROM triples WHERE p='http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/productFeature' AND o.kind=0),
product AS (SELECT s, o.value AS obj FROM triples WHERE p='http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/product' AND o.kind=0),
vendor AS (SELECT s, o.value AS obj FROM triples WHERE p='http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/vendor' AND o.kind=0),
reviewer AS (SELECT s, o.value AS obj FROM triples WHERE p='http://purl.org/stuff/rev#reviewer' AND o.kind=0),
price AS (SELECT s, o.value AS lex, o.datatype AS datatype, o.lang AS lang FROM triples WHERE p='http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/price' AND o.kind=2),
dates AS (SELECT s, o.value AS lex, o.datatype AS datatype, o.lang AS lang FROM triples WHERE p='http://purl.org/dc/elements/1.1/date' AND o.kind=2),
ratings AS (SELECT s AS review, TRY_CAST(o.value AS REAL) AS score FROM triples WHERE p='http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/rating1' AND o.kind=2 UNION ALL SELECT s AS review, TRY_CAST(o.value AS REAL) AS score FROM triples WHERE p='http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/rating2' AND o.kind=2 UNION ALL SELECT s AS review, TRY_CAST(o.value AS REAL) AS score FROM triples WHERE p='http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/rating3' AND o.kind=2 UNION ALL SELECT s AS review, TRY_CAST(o.value AS REAL) AS score FROM triples WHERE p='http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/rating4' AND o.kind=2),
producerReviews AS (SELECT rf.s AS review FROM producer p JOIN reviewFor rf ON rf.obj=p.s
WHERE p.obj='http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/instances/dataFromProducer59/Producer59'),
globalAvg AS (SELECT CASE WHEN COUNT(r.score)=COUNT(*) THEN CAST(AVG(r.score) AS REAL) END AS score
FROM producerReviews pr JOIN ratings r ON r.review=pr.review)
SELECT rr.obj AS reviewer,CASE WHEN COUNT(r.score)=COUNT(*) THEN CAST(AVG(r.score) AS REAL) END AS reviewerAvgScore
FROM producerReviews pr JOIN ratings r ON r.review=pr.review JOIN reviewer rr ON rr.s=pr.review CROSS JOIN globalAvg g
GROUP BY rr.obj HAVING CASE WHEN COUNT(r.score)=COUNT(*) THEN CAST(AVG(r.score) AS REAL) END>CAST(min(g.score)*CAST(1.5 AS REAL) AS REAL)