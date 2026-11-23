//! SPARQL 1.2 evaluation (feature `sparql-12`): triple-term patterns (ground, with variables, nested, with blank
//! nodes), the reified-triple and annotation syntax end to end, the triple-term and base-direction functions,
//! ORDER BY and `=` over triple terms and directional literals, VALUES and CONSTRUCT with triple terms, and INSERT /
//! DELETE DATA with triple terms through the update path.
#![cfg(feature = "rdf-12")]
use triplelight::exec::{query, QueryResult};
use triplelight::storage::MemorySnapshot;

const INT: &str = "<http://www.w3.org/2001/XMLSchema#integer>";

const DATA: &str = r#"<urn:alice> <urn:says> <<( <urn:bob> <urn:age> "23"^^<http://www.w3.org/2001/XMLSchema#integer> )>> .
<urn:carol> <urn:says> <<( <urn:bob> <urn:age> "24"^^<http://www.w3.org/2001/XMLSchema#integer> )>> .
<urn:dave> <urn:says> <<( <urn:alice> <urn:says> <<( <urn:bob> <urn:age> "23"^^<http://www.w3.org/2001/XMLSchema#integer> )>> )>> .
<urn:erin> <urn:says> <urn:plain> .
<urn:bob> <urn:name> "Bob" .
_:r <http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies> <<( <urn:bob> <urn:name> "Bob" )>> .
_:r <urn:source> <urn:census> .
"#;

fn store(nt: &str) -> MemorySnapshot { MemorySnapshot::from_ntriples(nt.as_bytes()).unwrap() }

/// Rows as display strings ("-" for unbound), in result order.
fn rows(s: &MemorySnapshot, q: &str) -> Vec<Vec<String>> {
    match query(s, q).unwrap() {
        QueryResult::Solutions { rows, .. } => rows.iter().map(|r| r.iter().map(|t| t.as_ref().map_or("-".into(), |t| t.to_string())).collect()).collect(),
        r => panic!("{r:?}"),
    }
}
fn sorted(mut v: Vec<Vec<String>>) -> Vec<Vec<String>> { v.sort(); v }
fn row(xs: &[&str]) -> Vec<String> { xs.iter().map(|x| x.to_string()).collect() }

/// `expr` evaluated once, as display text ("-" for an error).
fn one(expr: &str) -> String {
    let s = store("<urn:a> <urn:p> <urn:b> .\n");
    rows(&s, &format!("PREFIX xsd: <http://www.w3.org/2001/XMLSchema#> SELECT ({expr} AS ?x) WHERE {{}}")).remove(0).remove(0)
}
const T: &str = "\"true\"^^<http://www.w3.org/2001/XMLSchema#boolean>";
const F: &str = "\"false\"^^<http://www.w3.org/2001/XMLSchema#boolean>";

#[test]
fn ground_triple_term_pattern_is_a_constant() {
    let s = store(DATA);
    assert_eq!(rows(&s, "SELECT ?who WHERE { ?who <urn:says> <<( <urn:bob> <urn:age> 23 )>> }"), vec![row(&["<urn:alice>"])]);
    // Absent from the dictionary: no match (not an error).
    assert!(rows(&s, "SELECT ?who WHERE { ?who <urn:says> <<( <urn:bob> <urn:age> 99 )>> }").is_empty());
    // Nested ground triple term.
    assert_eq!(rows(&s, "SELECT ?who WHERE { ?who <urn:says> <<( <urn:alice> <urn:says> <<( <urn:bob> <urn:age> 23 )>> )>> }"), vec![row(&["<urn:dave>"])]);
    assert!(matches!(query(&s, "ASK { <urn:alice> <urn:says> <<( <urn:bob> <urn:age> 23 )>> }").unwrap(), QueryResult::Boolean(true)));
}

#[test]
fn variable_triple_term_patterns_unify() {
    let s = store(DATA);
    // Only triple terms with the right predicate match; the plain IRI object of erin and dave's nested term do not.
    assert_eq!(sorted(rows(&s, "SELECT ?who ?age WHERE { ?who <urn:says> <<( <urn:bob> <urn:age> ?age )>> }")),
        vec![row(&["<urn:alice>", &format!("\"23\"^^{INT}")]), row(&["<urn:carol>", &format!("\"24\"^^{INT}")])]);
    // SELECT * shows the nested variables, never the hidden slot variable.
    match query(&s, "SELECT * WHERE { ?who <urn:says> <<( ?s ?p ?o )>> }").unwrap() {
        QueryResult::Solutions { vars, rows } => {
            assert_eq!(vars.iter().map(|v| v.as_str()).collect::<Vec<_>>(), ["who", "s", "p", "o"]);
            assert_eq!(rows.len(), 3);
        }
        r => panic!("{r:?}"),
    }
    // Nested pattern with variables at two depths.
    assert_eq!(rows(&s, "SELECT ?who ?x ?age WHERE { ?who <urn:says> <<( ?x <urn:says> <<( <urn:bob> <urn:age> ?age )>> )>> }"),
        vec![row(&["<urn:dave>", "<urn:alice>", &format!("\"23\"^^{INT}")])]);
    // Variables bound elsewhere in the BGP must agree with the triple term's components.
    assert_eq!(rows(&s, "SELECT ?x ?who WHERE { ?x <urn:says> ?t . ?who <urn:says> <<( ?x <urn:says> ?t )>> }"),
        vec![row(&["<urn:alice>", "<urn:dave>"])]);
    assert!(rows(&s, "SELECT ?who WHERE { ?who <urn:says> <<( ?who ?p ?o )>> }").is_empty());
    // A blank node inside the pattern acts as a variable.
    assert_eq!(rows(&s, "SELECT ?who WHERE { ?who <urn:says> <<( _:b <urn:age> 24 )>> }"), vec![row(&["<urn:carol>"])]);
    // A FILTER over a nested variable is applied after unification.
    assert_eq!(rows(&s, "SELECT ?who WHERE { ?who <urn:says> <<( <urn:bob> <urn:age> ?age )>> FILTER(?age > 23) }"), vec![row(&["<urn:carol>"])]);
    // Counting goes through the general evaluator.
    assert_eq!(rows(&s, "SELECT (COUNT(*) AS ?n) WHERE { ?who <urn:says> <<( <urn:bob> ?p ?o )>> }"), vec![row(&[&format!("\"2\"^^{INT}")])]);
    // RDF 1.2 has no triple terms in subject position.
    assert!(rows(&s, "SELECT * WHERE { <<( <urn:bob> <urn:age> 23 )>> ?p ?o }").is_empty());
    assert!(rows(&s, "SELECT * WHERE { <<( ?s ?p ?o )>> ?q ?r }").is_empty());
}

#[test]
fn reified_triples_and_annotations_end_to_end() {
    let s = store(DATA);
    assert_eq!(rows(&s, "SELECT ?src WHERE { << <urn:bob> <urn:name> \"Bob\" >> <urn:source> ?src }"), vec![row(&["<urn:census>"])]);
    assert_eq!(rows(&s, "SELECT ?src WHERE { <urn:bob> <urn:name> \"Bob\" {| <urn:source> ?src |} }"), vec![row(&["<urn:census>"])]);
    assert_eq!(rows(&s, "SELECT ?r WHERE { <urn:bob> <urn:name> \"Bob\" ~ ?r . }").len(), 1);
    assert_eq!(rows(&s, "SELECT ?n WHERE { << <urn:bob> <urn:name> ?n >> <urn:source> <urn:census> }"), vec![row(&["\"Bob\""])]);
    assert!(rows(&s, "SELECT ?src WHERE { << <urn:bob> <urn:name> \"Robert\" >> <urn:source> ?src }").is_empty());
}

#[test]
fn triple_term_functions() {
    assert_eq!(one("TRIPLE(<urn:s>, <urn:p>, \"o\")"), "<<( <urn:s> <urn:p> \"o\" )>>");
    assert_eq!(one("<<( <urn:s> <urn:p> \"o\" )>>"), "<<( <urn:s> <urn:p> \"o\" )>>");
    assert_eq!(one("TRIPLE(\"s\", <urn:p>, 1)"), "-");
    assert_eq!(one("TRIPLE(<urn:s>, \"p\", 1)"), "-");
    assert_eq!(one("TRIPLE(TRIPLE(<urn:s>, <urn:p>, 1), <urn:p>, 1)"), "-");
    assert_eq!(one("OBJECT(TRIPLE(<urn:s>, <urn:p>, TRIPLE(<urn:a>, <urn:b>, <urn:c>)))"), "<<( <urn:a> <urn:b> <urn:c> )>>");
    assert_eq!(one("SUBJECT(<<( <urn:s> <urn:p> \"o\" )>>)"), "<urn:s>");
    assert_eq!(one("PREDICATE(<<( <urn:s> <urn:p> \"o\" )>>)"), "<urn:p>");
    assert_eq!(one("OBJECT(<<( <urn:s> <urn:p> \"o\" )>>)"), "\"o\"");
    assert_eq!(one("SUBJECT(<urn:s>)"), "-");
    assert_eq!(one("OBJECT(\"o\")"), "-");
    assert_eq!(one("isTRIPLE(<<( <urn:s> <urn:p> \"o\" )>>)"), T);
    assert_eq!(one("isTRIPLE(<urn:s>)"), F);
    assert_eq!(one("isLITERAL(<<( <urn:s> <urn:p> \"o\" )>>)"), F);
    assert_eq!(one("STR(<<( <urn:s> <urn:p> \"o\" )>>)"), "-");
}

#[test]
fn base_direction_functions() {
    assert_eq!(one("LANGDIR(\"abc\"@en--ltr)"), "\"ltr\"");
    assert_eq!(one("LANGDIR(\"abc\"@en--rtl)"), "\"rtl\"");
    assert_eq!(one("LANGDIR(\"abc\"@en)"), "\"\"");
    assert_eq!(one("LANGDIR(1)"), "\"\"");
    assert_eq!(one("LANGDIR(<urn:x>)"), "-");
    assert_eq!(one("hasLANG(\"abc\"@en)"), T);
    assert_eq!(one("hasLANG(\"abc\"@en--rtl)"), T);
    assert_eq!(one("hasLANG(\"abc\")"), F);
    assert_eq!(one("hasLANG(<urn:x>)"), F);
    assert_eq!(one("hasLANGDIR(\"abc\"@en--rtl)"), T);
    assert_eq!(one("hasLANGDIR(\"abc\"@en)"), F);
    assert_eq!(one("hasLANGDIR(1)"), F);
    assert_eq!(one("STRLANGDIR(\"abc\", \"en\", \"rtl\")"), "\"abc\"@en--rtl");
    assert_eq!(one("STRLANGDIR(\"abc\", \"EN\", \"ltr\")"), "\"abc\"@en--ltr");
    assert_eq!(one("STRLANGDIR(\"abc\", \"en\", \"LTR\")"), "-");
    assert_eq!(one("STRLANGDIR(\"abc\", \"en\", \"\")"), "-");
    assert_eq!(one("STRLANGDIR(\"abc\", \"\", \"ltr\")"), "-");
    assert_eq!(one("STRLANGDIR(123, \"en\", \"ltr\")"), "-");
    assert_eq!(one("STRLANGDIR(<urn:x>, \"en\", \"ltr\")"), "-");
    assert_eq!(one("LANG(\"abc\"@en--ltr)"), "\"en\"");
    assert_eq!(one("DATATYPE(\"abc\"@en--ltr)"), "<http://www.w3.org/1999/02/22-rdf-syntax-ns#dirLangString>");
    assert_eq!(one("UCASE(\"abc\"@en--rtl)"), "\"ABC\"@en--rtl");
}

#[test]
fn equality_of_triple_terms_and_directional_literals() {
    // `=` on triple terms is component-wise value equality; sameTerm is term identity.
    assert_eq!(one("<<( <urn:s> <urn:p> 123 )>> = <<( <urn:s> <urn:p> 123.0 )>>"), T);
    assert_eq!(one("sameTerm(<<( <urn:s> <urn:p> 123 )>>, <<( <urn:s> <urn:p> 123.0 )>>)"), F);
    assert_eq!(one("<<( <urn:s> <urn:p> 1 )>> = <<( <urn:s> <urn:p> 2 )>>"), F);
    assert_eq!(one("<<( <urn:s> <urn:p> 1 )>> = <urn:s>"), F);
    assert_eq!(one("<<( <urn:s> <urn:p> 1 )>> != <<( <urn:s> <urn:q> 1 )>>"), T);
    assert_eq!(one("\"a\"@en--ltr = \"a\"@en--ltr"), T);
    assert_eq!(one("\"a\"@en--ltr = \"a\"@en"), F);
    assert_eq!(one("\"a\"@en--ltr = \"a\"@en--rtl"), F);
    assert_eq!(one("sameTerm(\"a\"@en--rtl, \"a\"@en)"), F);
}

#[test]
fn order_by_triple_terms_and_directional_literals() {
    let s = store(r#"<urn:o1> <urn:k> <<( <urn:b> <urn:p> "2" )>> .
<urn:o2> <urn:k> <<( <urn:a> <urn:p> "9" )>> .
<urn:o3> <urn:k> <<( <urn:a> <urn:p> "1" )>> .
<urn:o4> <urn:k> "z" .
<urn:o5> <urn:k> <urn:iri> .
<urn:o6> <urn:k> _:x .
"#);
    let asc: Vec<String> = rows(&s, "SELECT ?o WHERE { ?s <urn:k> ?o } ORDER BY ?o").into_iter().map(|mut r| r.remove(0)).collect();
    // blank < IRI < literal < triple term; triple terms component-wise.
    assert!(asc[0].starts_with("_:"), "{asc:?}");
    assert_eq!(&asc[1..], ["<urn:iri>", "\"z\"", "<<( <urn:a> <urn:p> \"1\" )>>", "<<( <urn:a> <urn:p> \"9\" )>>", "<<( <urn:b> <urn:p> \"2\" )>>"]);
    let desc: Vec<String> = rows(&s, "SELECT ?o WHERE { ?s <urn:k> ?o } ORDER BY DESC(?o) LIMIT 2").into_iter().map(|mut r| r.remove(0)).collect();
    assert_eq!(desc, ["<<( <urn:b> <urn:p> \"2\" )>>", "<<( <urn:a> <urn:p> \"9\" )>>"]);
    let s = store("<urn:a> <urn:k> \"a\"@en .\n<urn:b> <urn:k> \"a\"@en--rtl .\n<urn:c> <urn:k> \"a\"@en--ltr .\n");
    let lits: Vec<String> = rows(&s, "SELECT ?o WHERE { ?s <urn:k> ?o } ORDER BY ?o").into_iter().map(|mut r| r.remove(0)).collect();
    assert_eq!(lits, ["\"a\"@en--ltr", "\"a\"@en--rtl", "\"a\"@en"]);
    // A directional literal is a value of its own (no tie with the undirected one).
    assert_eq!(rows(&s, "SELECT (COUNT(DISTINCT ?o) AS ?n) WHERE { ?s <urn:k> ?o }"), vec![row(&[&format!("\"3\"^^{INT}")])]);
}

#[test]
fn values_with_triple_terms() {
    let s = store(DATA);
    assert_eq!(rows(&s, "SELECT ?who WHERE { VALUES ?t { <<( <urn:bob> <urn:age> 23 )>> } ?who <urn:says> ?t }"), vec![row(&["<urn:alice>"])]);
    assert_eq!(rows(&s, "SELECT ?t WHERE { VALUES ?t { <<( <urn:x> <urn:y> 77 )>> } }"), vec![row(&[&format!("<<( <urn:x> <urn:y> \"77\"^^{INT} )>>")])]);
    assert!(rows(&s, "SELECT ?who WHERE { VALUES ?t { <<( <urn:x> <urn:y> 77 )>> } ?who <urn:says> ?t }").is_empty());
    // A VALUES-bound nested variable restricts the triple-term match.
    assert_eq!(rows(&s, "SELECT ?who WHERE { VALUES ?age { 24 } ?who <urn:says> <<( <urn:bob> <urn:age> ?age )>> }"), vec![row(&["<urn:carol>"])]);
    // BIND of a triple term joins with stored ones.
    assert_eq!(rows(&s, "SELECT ?who WHERE { BIND(TRIPLE(<urn:bob>, <urn:age>, 24) AS ?t) ?who <urn:says> ?t }"), vec![row(&["<urn:carol>"])]);
}

#[test]
fn construct_emits_triple_terms() {
    let s = store(DATA);
    let QueryResult::Graph(mut g) = query(&s, "CONSTRUCT { ?who <urn:claims> <<( <urn:bob> <urn:age> ?age )>> } WHERE { ?who <urn:says> <<( <urn:bob> <urn:age> ?age )>> }").unwrap() else { panic!() };
    g.sort_by_key(|t| t.to_string());
    let text: Vec<String> = g.iter().map(|t| t.to_string()).collect();
    assert_eq!(text, [
        format!("<urn:alice> <urn:claims> <<( <urn:bob> <urn:age> \"23\"^^{INT} )>>"),
        format!("<urn:carol> <urn:claims> <<( <urn:bob> <urn:age> \"24\"^^{INT} )>>"),
    ]);
    // Template blank nodes inside a triple term are fresh per solution.
    let QueryResult::Graph(g) = query(&s, "CONSTRUCT { ?who <urn:claims> <<( _:x <urn:age> ?age )>> } WHERE { ?who <urn:says> <<( <urn:bob> <urn:age> ?age )>> }").unwrap() else { panic!() };
    assert_eq!(g.len(), 2);
    let subjects: std::collections::HashSet<String> = g.iter().map(|t| match &t.object {
        triplelight::rdf::Term::Triple(tt) => { assert!(tt.subject.is_blank_node()); tt.subject.to_string() }
        o => panic!("{o}"),
    }).collect();
    assert_eq!(subjects.len(), 2);
}

#[cfg(unix)]
#[test]
fn insert_and_delete_data_with_triple_terms() {
    use triplelight::embedded::Database;
    let db = Database::in_memory();
    let n = |db: &Database, q: &str| match db.query(q).unwrap() { QueryResult::Solutions { rows, .. } => rows.len(), r => panic!("{r:?}") };
    assert_eq!(db.update("INSERT DATA { <urn:a> <urn:says> <<( <urn:b> <urn:p> 1 )>> . <urn:a> <urn:says> <<( <urn:b> <urn:q> _:x )>> }").unwrap(), 2);
    assert_eq!(n(&db, "SELECT ?o WHERE { <urn:a> <urn:says> <<( <urn:b> <urn:p> ?o )>> }"), 1);
    assert_eq!(n(&db, "SELECT ?o WHERE { <urn:a> <urn:says> <<( <urn:b> <urn:q> ?o )>> FILTER(isBLANK(?o)) }"), 1);
    // INSERT ... WHERE builds triple terms from solutions; DELETE DATA removes a ground one.
    db.update("INSERT { ?s <urn:copy> <<( ?s <urn:said> ?t )>> } WHERE { ?s <urn:says> ?t }").unwrap();
    assert_eq!(n(&db, "SELECT ?t WHERE { <urn:a> <urn:copy> <<( <urn:a> <urn:said> ?t )>> }"), 2);
    assert_eq!(db.update("DELETE DATA { <urn:a> <urn:says> <<( <urn:b> <urn:p> 1 )>> }").unwrap(), 1);
    assert_eq!(n(&db, "SELECT ?o WHERE { <urn:a> <urn:says> <<( <urn:b> <urn:p> ?o )>> }"), 0);
    assert_eq!(db.update("DELETE { ?s <urn:copy> <<( ?s <urn:said> ?t )>> } WHERE { ?s <urn:copy> <<( ?s <urn:said> ?t )>> }").unwrap(), 2);
}
