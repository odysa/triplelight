//! SPARQL Update over named graphs and sequential operation semantics (task #36).
use triplelight::embedded::Database;
use triplelight::exec::QueryResult;

const P: &str = "PREFIX : <http://ex/> ";

fn rows(db: &Database, q: &str) -> Vec<Vec<String>> {
    match db.query(&format!("{P}{q}")).unwrap() {
        QueryResult::Solutions { rows, .. } => {
            let mut v: Vec<Vec<String>> = rows.into_iter().map(|r| r.into_iter().map(|t| t.map(|t| t.to_string()).unwrap_or("-".into())).collect()).collect();
            v.sort();
            v
        }
        r => panic!("{r:?}"),
    }
}
fn update(db: &Database, u: &str) -> usize { db.update(&format!("{P}{u}")).unwrap() }
fn one(s: &str) -> Vec<Vec<String>> { vec![vec![s.to_string()]] }
const ONE: &str = "\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>";

#[test]
fn insert_and_delete_data_in_a_named_graph() {
    let db = Database::in_memory();
    assert_eq!(update(&db, "INSERT DATA { GRAPH :g { :a :p 1 } :a :p 2 }"), 2);
    assert_eq!(rows(&db, "SELECT ?o { GRAPH :g { :a :p ?o } }"), one(ONE));
    assert_eq!(rows(&db, "SELECT ?o { :a :p ?o }"), one("\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>"));
    assert_eq!(rows(&db, "SELECT ?g { GRAPH ?g { } }"), one("<http://ex/g>"));
    assert_eq!(update(&db, "DELETE DATA { GRAPH :g { :a :p 1 } }"), 1);
    assert_eq!(rows(&db, "SELECT ?o { GRAPH :g { :a :p ?o } }"), Vec::<Vec<String>>::new());
    // Deleting a graph's last triple keeps the (now empty) graph in the catalog.
    assert_eq!(rows(&db, "SELECT ?g { GRAPH ?g { } }"), one("<http://ex/g>"));
}

#[test]
fn delete_insert_with_graph_templates_with_and_using() {
    let db = Database::in_memory();
    update(&db, "INSERT DATA { GRAPH :g1 { :a :p 1 } GRAPH :g2 { :b :p 1 } }");
    // A graph variable bound by the WHERE clause targets each solution's graph.
    update(&db, "INSERT { GRAPH ?g { ?s :q ?o } } WHERE { GRAPH ?g { ?s :p ?o } }");
    assert_eq!(rows(&db, "SELECT ?g ?s { GRAPH ?g { ?s :q ?o } }"),
               vec![vec!["<http://ex/g1>".to_string(), "<http://ex/a>".to_string()], vec!["<http://ex/g2>".to_string(), "<http://ex/b>".to_string()]]);
    // WITH: the WHERE default graph and the template target are :g1.
    update(&db, "WITH :g1 DELETE { ?s :q ?o } INSERT { ?s :r ?o } WHERE { ?s :q ?o }");
    assert_eq!(rows(&db, "SELECT ?s { GRAPH :g1 { ?s :r ?o } }"), one("<http://ex/a>"));
    assert_eq!(rows(&db, "SELECT ?s { GRAPH :g1 { ?s :q ?o } }"), Vec::<Vec<String>>::new());
    assert_eq!(rows(&db, "SELECT ?s { GRAPH :g2 { ?s :q ?o } }"), one("<http://ex/b>"), "WITH does not touch other graphs");
    // USING: the WHERE clause reads :g2 as its default graph; templates target the store's default graph.
    update(&db, "INSERT { ?s :from2 ?o } USING :g2 WHERE { ?s :p ?o }");
    assert_eq!(rows(&db, "SELECT ?s { ?s :from2 ?o }"), one("<http://ex/b>"));
}

#[test]
fn each_operation_sees_the_effects_of_the_previous_ones() {
    let db = Database::in_memory();
    update(&db, "INSERT DATA { :a :p 1 } ; INSERT { :a :q ?o } WHERE { :a :p ?o }");
    assert_eq!(rows(&db, "SELECT ?o { :a :q ?o }"), one(ONE));
    update(&db, "DELETE DATA { :a :p 1 } ; INSERT { :a :gone ?o } WHERE { :a :p ?o }");
    assert_eq!(rows(&db, "SELECT ?o { :a :gone ?o }"), Vec::<Vec<String>>::new());
}

#[test]
fn deletions_of_one_operation_precede_its_insertions() {
    let db = Database::in_memory();
    update(&db, "INSERT DATA { :a :p 1 . :b :p 1 }");
    // Row (a, b) deletes :a and inserts :b; row (b, a) deletes :b and inserts :a. Applying rows one by one would leave
    // only one of them; deleting first (the SPARQL semantics) keeps both.
    update(&db, "DELETE { ?s :p 1 } INSERT { ?t :p 1 } WHERE { ?s :p 1 . ?t :p 1 FILTER(?s != ?t) }");
    assert_eq!(rows(&db, "SELECT ?s { ?s :p 1 }"), vec![vec!["<http://ex/a>".to_string()], vec!["<http://ex/b>".to_string()]]);
}

#[test]
fn blank_nodes_are_fresh_per_operation_and_shared_within_one() {
    let db = Database::in_memory();
    let ins = "INSERT DATA { GRAPH :g { _:x :p 1 } GRAPH :h { _:x :p 2 } }";
    update(&db, ins);
    update(&db, ins);
    // Two runs: two distinct nodes in :g; within one run, _:x in :g and :h is one node.
    assert_eq!(rows(&db, "SELECT (COUNT(DISTINCT ?x) AS ?n) { GRAPH :g { ?x :p 1 } }"), one("\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>"));
    assert_eq!(rows(&db, "SELECT (COUNT(*) AS ?n) { GRAPH :g { ?x :p 1 } GRAPH :h { ?x :p 2 } }"), one("\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>"));
    // Templates: a fresh node per solution and per operation.
    update(&db, "INSERT { GRAPH :t { _:y :of ?x } } WHERE { GRAPH :g { ?x :p 1 } }");
    update(&db, "INSERT { GRAPH :t { _:y :of ?x } } WHERE { GRAPH :g { ?x :p 1 } }");
    assert_eq!(rows(&db, "SELECT (COUNT(DISTINCT ?y) AS ?n) { GRAPH :t { ?y :of ?x } }"), one("\"4\"^^<http://www.w3.org/2001/XMLSchema#integer>"));
}

#[test]
fn persistent_store_keeps_named_graph_updates_across_reopen() {
    let dir = std::env::temp_dir().join(format!("tl-dataset-updates-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = Database::open(&dir).unwrap();
    update(&db, "INSERT DATA { GRAPH :g { :a :p 1 } } ; INSERT DATA { :a :p 1 }");
    update(&db, "INSERT { GRAPH :h { ?s :q ?o } } WHERE { GRAPH :g { ?s :p ?o } }");
    update(&db, "DELETE DATA { :a :p 1 } ; INSERT { :a :r ?o } WHERE { GRAPH :h { :a :q ?o } }");
    drop(db);
    let db = Database::open(&dir).unwrap();
    assert_eq!(rows(&db, "SELECT ?o { GRAPH :g { :a :p ?o } }"), one(ONE));
    assert_eq!(rows(&db, "SELECT ?o { GRAPH :h { :a :q ?o } }"), one(ONE));
    assert_eq!(rows(&db, "SELECT ?o { :a :r ?o }"), one(ONE));
    assert_eq!(rows(&db, "SELECT ?o { :a :p ?o }"), Vec::<Vec<String>>::new());
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn reported_count_is_the_sum_of_the_operations_on_both_backends() {
    let dir = std::env::temp_dir().join(format!("tl-dataset-count-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for db in [Database::in_memory(), Database::open(&dir).unwrap()] {
        assert_eq!(update(&db, "INSERT DATA { :x :p 1 } ; DELETE DATA { :x :p 1 }"), 2);
        assert_eq!(rows(&db, "SELECT ?s { ?s :p 1 }"), Vec::<Vec<String>>::new());
    }
    std::fs::remove_dir_all(dir).unwrap();
}

/// `Quads ::= TriplesTemplate? (QuadsNotTriples '.'? TriplesTemplate?)*`: a dot may end the triples before a
/// GRAPH block and may follow the block, with or without triples after it.
#[test]
fn a_dot_may_precede_and_follow_a_graph_block() {
    let two = "\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>";
    for data in [
        "INSERT DATA { :a :p 2 . GRAPH :g { :a :p 1 } }",
        "INSERT DATA { :a :p 2 . GRAPH :g { :a :p 1 } . }",
        "INSERT DATA { :a :p 2 . GRAPH :g { :a :p 1 . } . }",
        "INSERT DATA { :a :p 2 GRAPH :g { :a :p 1 } }",
        "INSERT DATA { GRAPH :g { :a :p 1 } . :a :p 2 }",
        "INSERT DATA { GRAPH :g { :a :p 1 } :a :p 2 . }",
        "INSERT DATA { :a :p 2 . GRAPH :g { :a :p 1 } . GRAPH :h { } }",
    ] {
        let db = Database::in_memory();
        update(&db, data);
        assert_eq!(rows(&db, "SELECT ?o { GRAPH :g { :a :p ?o } }"), one(ONE), "{data}");
        assert_eq!(rows(&db, "SELECT ?o { :a :p ?o }"), one(two), "{data}");
    }
}

#[test]
fn a_dot_before_a_graph_block_in_delete_data_and_templates() {
    let db = Database::in_memory();
    update(&db, "INSERT DATA { :a :p 2 . GRAPH :g { :a :p 1 } . :b :p 3 }");
    // DELETE DATA with a default-graph triple, a dot, then a GRAPH block.
    update(&db, "DELETE DATA { :b :p 3 . GRAPH :g { :a :p 1 } }");
    assert_eq!(rows(&db, "SELECT ?o { GRAPH :g { :a :p ?o } }"), Vec::<Vec<String>>::new());
    assert_eq!(rows(&db, "SELECT ?s { ?s :p 3 }"), Vec::<Vec<String>>::new());
    // DELETE/INSERT templates.
    update(&db, "DELETE { ?s :p ?o . GRAPH :g { ?s :p ?o } } INSERT { ?s :q ?o . GRAPH :h { ?s :q ?o } } WHERE { ?s :p ?o }");
    assert_eq!(rows(&db, "SELECT ?s { :a :p ?o }"), Vec::<Vec<String>>::new());
    assert_eq!(rows(&db, "SELECT ?o { :a :q ?o }").len(), 1);
    assert_eq!(rows(&db, "SELECT ?o { GRAPH :h { :a :q ?o } }").len(), 1);
}

#[test]
fn graph_still_needs_a_term_or_close_after_a_dot() {
    let db = Database::in_memory();
    assert!(db.update(&format!("{P}INSERT DATA {{ :a :p 1 . GRAPH }}")).is_err());
    assert!(db.update(&format!("{P}INSERT DATA {{ :a :p 1 . . }}")).is_err());
}

#[test]
fn load_resolver_preserves_order_graphs_and_atomicity() {
    use triplelight::embedded::DatabaseError;
    use triplelight::rdf::{BlankNode, Literal, NamedNode, Triple};

    let dir = std::env::temp_dir().join(format!("tl-load-update-{}", std::process::id()));
    let load = |source: &NamedNode| match source.as_str() {
        "urn:good" => Ok(vec![Triple::new(
            BlankNode::new_unchecked("doc"),
            NamedNode::new_unchecked("urn:p"),
            Literal::new_simple_literal("loaded"),
        )]),
        "https://example.org/data.ttl" => Ok(vec![Triple::new(
            NamedNode::new_unchecked("urn:default"),
            NamedNode::new_unchecked("urn:p"),
            Literal::new_simple_literal("resolved"),
        )]),
        "urn:empty" => Ok(Vec::new()),
        _ => Err(DatabaseError::Storage(std::io::Error::from(std::io::ErrorKind::NotFound))),
    };
    let check = |db: &Database| {
        assert!(matches!(db.query("ASK { GRAPH <urn:empty> {} }").unwrap(), QueryResult::Boolean(true)));
        assert_eq!(
            rows(db, "SELECT ?s WHERE { ?s <urn:p> 'resolved' }"),
            one("<urn:default>"),
        );
        let solutions = rows(db, "SELECT ?s WHERE { ?s <urn:q> 'seen' }");
        assert_eq!(solutions.len(), 2);
        assert_ne!(solutions[0], solutions[1]);
    };

    for db in [Database::in_memory(), Database::open(&dir).unwrap()] {
        assert!(db.update_with_loader(
            "INSERT DATA { <urn:s> <urn:p> 'partial' }; LOAD <urn:missing>",
            load,
        ).is_err());
        assert!(matches!(db.query("ASK { ?s ?p ?o }").unwrap(), QueryResult::Boolean(false)));

        let changed = db.update_with_loader(
            "BASE <https://example.org/> LOAD <data.ttl>; \
             LOAD SILENT <urn:missing>; LOAD <urn:empty> INTO GRAPH <urn:empty>; \
             LOAD <urn:good> INTO GRAPH <urn:g>; LOAD <urn:good> INTO GRAPH <urn:g>; \
             INSERT { ?s <urn:q> 'seen' } WHERE { GRAPH <urn:g> { ?s <urn:p> 'loaded' } }",
            load,
        ).unwrap();
        assert_eq!(changed, 5);
        check(&db);
    }
    check(&Database::open(&dir).unwrap());
    std::fs::remove_dir_all(dir).unwrap();
}
