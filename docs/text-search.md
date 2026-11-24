# Match words in RDF literals

Use the TripleLight extension function `tl:matches` in a SPARQL expression.
The RDF pattern supplies the candidates, and the function tests the lexical
form of each candidate.

```sparql
PREFIX tl: <urn:triplelight:text:>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?item ?label WHERE {
  ?item rdfs:label ?label .
  FILTER(tl:matches(?label, "graph database"))
}
```

Both arguments accept plain strings and language-tagged strings. Language
tags remain available through ordinary SPARQL expressions. The function
compares whole words after Unicode 17.0.0 NFKC case folding and default
word segmentation. Every query word must appear. Word order and repeated
query words have no effect. A search string with no words evaluates to
false. Other argument types, unbound arguments, and an incorrect argument
count produce a SPARQL expression error.

Default word boundaries can give coarse results in languages that depend
on dictionary-based segmentation. This version evaluates the function
on each candidate literal.
