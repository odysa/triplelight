use triplelight::exec::{QueryErrorKind, QueryResult, query};
use triplelight::storage::MemorySnapshot;

const PREFIX: &str = "PREFIX tl: <urn:triplelight:text:>";

fn ask(filter: &str) -> bool {
    let snapshot = MemorySnapshot::from_ntriples(&b""[..]).unwrap();
    let text = format!("{PREFIX} ASK {{ FILTER({filter}) }}");
    let QueryResult::Boolean(answer) = query(&snapshot, &text).unwrap() else {
        panic!("ASK must return a boolean");
    };
    answer
}

#[test]
fn text_matches_all_whole_words_after_unicode_folding() {
    assert!(ask(r#"tl:matches("Straße, ＧＲＡＰＨ", "graph STRASSE graph")"#));
    assert!(ask(r#"tl:matches("cafe\u0301", "CAFÉ")"#));
    assert!(ask(r#"tl:matches("da\u00ADta", "DATA")"#));
    assert!(ask(r#"tl:matches("Graph Database"@en, "database graph"@de)"#));
    assert!(!ask(r#"tl:matches("database", "data")"#));
    assert!(!ask(r#"tl:matches("graph database", "graph query")"#));
    assert!(!ask(r#"tl:matches("graph database", "?!")"#));
}

#[test]
fn text_matches_argument_errors_are_sparql_expression_errors() {
    assert!(!ask(r#"!tl:matches(42, "42")"#));
    assert!(!ask(r#"!tl:matches(?missing, "text")"#));
    assert!(!ask(r#"!tl:matches("text")"#));
    assert!(!ask(r#"!tl:matches("text", "text", "extra")"#));

    let snapshot = MemorySnapshot::from_ntriples(&b""[..]).unwrap();
    let error = query(&snapshot, "ASK { FILTER(<urn:other:matches>(\"x\", \"x\")) }").unwrap_err();
    assert_eq!(error.kind, QueryErrorKind::Unsupported);
}
