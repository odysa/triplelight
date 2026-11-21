//! SPARQL Update over named graphs and sequential operation semantics (task #36).
use triplelight::embedded::{Database, DatabaseError};
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
    // Solution order must not matter: the template deletes and re-inserts the same triple for different rows.
    update(&db, "DELETE { :a :p 1 } INSERT { :a :p 1 } WHERE { ?s :p 1 }");
    assert_eq!(rows(&db, "SELECT ?s { ?s :p 1 }"), vec![vec!["<http://ex/a>".to_string()], vec!["<http://ex/b>".to_string()]]);
}

#[test]
fn persistent_store_refuses_named_graphs_but_runs_sequential_updates() {
    let dir = std::env::temp_dir().join(format!("tl-dataset-updates-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = Database::open(&dir).unwrap();
    match db.update(&format!("{P}INSERT DATA {{ GRAPH :g {{ :a :p 1 }} }}")) {
        Err(DatabaseError::Unsupported(_)) => {}
        other => panic!("expected Unsupported, got {other:?}"),
    }
    assert_eq!(rows(&db, "SELECT ?o { GRAPH ?g { ?s ?p ?o } }"), Vec::<Vec<String>>::new());
    update(&db, "INSERT DATA { :a :p 1 } ; INSERT { :a :q ?o } WHERE { :a :p ?o }");
    drop(db);
    let db = Database::open(&dir).unwrap();
    assert_eq!(rows(&db, "SELECT ?o { :a :q ?o }"), one(ONE));
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}
