#![cfg(feature = "rdfs")]

use triplelight::embedded::Database;
use triplelight::exec::{Entailment, QueryResult};
use triplelight::extension::OwlRlEquivalence;

const EXTENSION: &str = "owl-rl-equivalence";

#[test]
fn named_equivalences_feed_the_rdfs_view() {
    let db = Database::in_memory();
    db.update(
        "PREFIX owl: <http://www.w3.org/2002/07/owl#>
         PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
         INSERT DATA {
             <urn:A> owl:equivalentClass <urn:B> .
             <urn:B> owl:equivalentClass <urn:C> .
             <urn:p> owl:equivalentProperty <urn:q> .
             <urn:q> owl:equivalentProperty <urn:r> .
             <urn:r> rdfs:domain <urn:C> .
             <urn:s> <urn:p> <urn:o> .
             <urn:s> <urn:q> <urn:o> .
             <urn:t> <urn:r> <urn:o> .
         }",
    )
    .unwrap();
    db.register_extension(&OwlRlEquivalence).unwrap();

    let inferred = "ASK { <urn:s> a <urn:A> ; <urn:r> <urn:o> .
                         <urn:t> a <urn:C> ; <urn:p> <urn:o> }";
    assert!(matches!(
        db.query_with_extension(inferred, EXTENSION).unwrap(),
        QueryResult::Boolean(true)
    ));
    for entailment in [Entailment::Simple, Entailment::Rdfs] {
        assert!(matches!(
            db.query_with_entailment(inferred, entailment).unwrap(),
            QueryResult::Boolean(false)
        ));
    }

    let schema_and_duplicates = "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
        ASK {
            <urn:C> ?class <urn:A> .
            <urn:r> ?property <urn:p> .
            { SELECT (COUNT(*) AS ?n) WHERE { <urn:s> <urn:q> <urn:o> } }
            FILTER (?class = rdfs:subClassOf && ?property = rdfs:subPropertyOf && ?n = 1)
        }";
    assert!(matches!(
        db.query_with_extension(schema_and_duplicates, EXTENSION)
            .unwrap(),
        QueryResult::Boolean(true)
    ));
}

#[test]
fn equivalence_is_graph_scoped_and_requires_named_application_properties() {
    let db = Database::in_memory();
    db.update(
        "PREFIX owl: <http://www.w3.org/2002/07/owl#>
         PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
         INSERT DATA {
             GRAPH <urn:schema> {
                 <urn:p> owl:equivalentProperty <urn:q>, rdf:type, _:b, \"urn:literal\" .
             }
             GRAPH <urn:data> { <urn:s> <urn:p> <urn:o> . }
         }",
    )
    .unwrap();
    db.register_extension(&OwlRlEquivalence).unwrap();

    for (query, expected) in [
        (
            "ASK { GRAPH <urn:data> { <urn:s> <urn:q> <urn:o> } }",
            false,
        ),
        (
            "ASK FROM <urn:schema> FROM <urn:data> { <urn:s> <urn:q> <urn:o> }",
            true,
        ),
        (
            "ASK FROM <urn:schema> FROM <urn:data> { <urn:s> a <urn:o> }",
            false,
        ),
        (
            "ASK FROM <urn:schema> FROM <urn:data> {
             <urn:p> <http://www.w3.org/2000/01/rdf-schema#subPropertyOf> ?p .
             FILTER (!isIRI(?p))
         }",
            false,
        ),
    ] {
        assert!(
            matches!(
                db.query_with_extension(query, EXTENSION).unwrap(),
                QueryResult::Boolean(value) if value == expected
            ),
            "{query}"
        );
    }
}
