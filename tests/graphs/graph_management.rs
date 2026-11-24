//! SPARQL 1.1 Update graph management: CREATE, DROP, CLEAR, ADD, MOVE, COPY and LOAD (task #36).
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
fn update(db: &Database, u: &str) { db.update(&format!("{P}{u}")).unwrap(); }
fn fails(db: &Database, u: &str) -> DatabaseError { db.update(&format!("{P}{u}")).unwrap_err() }
fn graphs(db: &Database) -> Vec<Vec<String>> { rows(db, "SELECT ?g { GRAPH ?g { } }") }
fn objects(db: &Database, graph: Option<&str>) -> Vec<Vec<String>> {
    match graph {
        Some(g) => rows(db, &format!("SELECT ?o {{ GRAPH {g} {{ :a :p ?o }} }}")),
        None => rows(db, "SELECT ?o { :a :p ?o }"),
    }
}
fn lit(n: u32) -> Vec<String> { vec![format!("\"{n}\"^^<http://www.w3.org/2001/XMLSchema#integer>")] }
fn g(names: &[&str]) -> Vec<Vec<String>> { names.iter().map(|n| vec![format!("<http://ex/{n}>")]).collect() }
const NONE: Vec<Vec<String>> = Vec::new();

/// Runs `f` on the in-memory database and on a fresh persistent store (its catalog and WAL records).
fn backends(name: &str, f: impl Fn(&Database)) {
    f(&Database::in_memory());
    let dir = std::env::temp_dir().join(format!("tl-graph-mgmt-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    f(&Database::open(&dir).unwrap());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn create_adds_an_empty_graph_and_fails_on_an_existing_one_unless_silent() {
    backends("create", |db| {
        update(db, "CREATE GRAPH :g");
        assert_eq!(graphs(db), g(&["g"]));
        assert!(matches!(fails(db, "CREATE GRAPH :g"), DatabaseError::GraphState(_)));
        update(db, "CREATE SILENT GRAPH :g");
        assert_eq!(graphs(db), g(&["g"]));
    });
}

#[test]
fn clear_empties_and_drop_removes_with_absent_graph_errors() {
    backends("clear-drop", |db| {
        update(db, "INSERT DATA { GRAPH :g { :a :p 1 } GRAPH :h { :a :p 2 } :a :p 3 }");
        update(db, "CLEAR GRAPH :g");
        assert_eq!(graphs(db), g(&["g", "h"]));
        assert_eq!(objects(db, Some(":g")), NONE);
        update(db, "DROP GRAPH :g");
        assert_eq!(graphs(db), g(&["h"]));
        assert!(matches!(fails(db, "DROP GRAPH :g"), DatabaseError::GraphState(_)));
        assert!(matches!(fails(db, "CLEAR GRAPH :g"), DatabaseError::GraphState(_)));
        update(db, "DROP SILENT GRAPH :g ; CLEAR SILENT GRAPH :g");
        // DROP DEFAULT only empties the default graph, which always exists; named graphs are untouched.
        update(db, "DROP DEFAULT");
        assert_eq!(objects(db, None), NONE);
        assert_eq!(objects(db, Some(":h")), vec![lit(2)]);
    });
}

#[test]
fn clear_and_drop_named_and_all() {
    backends("named-all", |db| {
        update(db, "INSERT DATA { GRAPH :g { :a :p 1 } GRAPH :h { :a :p 2 } :a :p 3 }");
        update(db, "CLEAR NAMED");
        assert_eq!(graphs(db), g(&["g", "h"]));
        assert_eq!(objects(db, None), vec![lit(3)]);
        update(db, "INSERT DATA { GRAPH :g { :a :p 1 } }");
        update(db, "DROP NAMED");
        assert_eq!(graphs(db), NONE);
        assert_eq!(objects(db, None), vec![lit(3)]);
        update(db, "INSERT DATA { GRAPH :g { :a :p 1 } }");
        update(db, "CLEAR ALL");
        assert_eq!(graphs(db), g(&["g"]));
        assert_eq!(objects(db, None), NONE);
        update(db, "INSERT DATA { GRAPH :g { :a :p 1 } :a :p 3 }");
        update(db, "DROP ALL");
        assert_eq!(graphs(db), NONE);
        assert_eq!(objects(db, None), NONE);
    });
}

#[test]
fn add_copy_and_move_between_named_and_default_graphs() {
    backends("transfer", |db| {
        update(db, "INSERT DATA { GRAPH :g { :a :p 1 } GRAPH :h { :a :p 2 } :a :p 3 }");
        // ADD keeps the destination's triples.
        update(db, "ADD :g TO :h");
        assert_eq!(objects(db, Some(":h")), vec![lit(1), lit(2)]);
        // COPY replaces them, and creates an absent destination.
        update(db, "COPY DEFAULT TO :h ; COPY :g TO :k");
        assert_eq!(objects(db, Some(":h")), vec![lit(3)]);
        assert_eq!(objects(db, Some(":k")), vec![lit(1)]);
        // MOVE replaces the destination and drops the source.
        update(db, "MOVE :k TO DEFAULT");
        assert_eq!(objects(db, None), vec![lit(1)]);
        assert_eq!(graphs(db), g(&["g", "h"]));
        // MOVE from the default graph empties it (it cannot be removed).
        update(db, "MOVE DEFAULT TO GRAPH :m");
        assert_eq!(objects(db, None), NONE);
        assert_eq!(objects(db, Some(":m")), vec![lit(1)]);
    });
}

#[test]
fn transfer_from_an_absent_graph_fails_unless_silent_and_to_itself_does_nothing() {
    backends("transfer-edge", |db| {
        update(db, "INSERT DATA { GRAPH :g { :a :p 1 } }");
        for op in ["ADD", "COPY", "MOVE"] {
            assert!(matches!(fails(db, &format!("{op} :absent TO :g")), DatabaseError::GraphState(_)), "{op}");
            update(db, &format!("{op} SILENT :absent TO :g"));
            update(db, &format!("{op} :g TO :g"));
            // A self-transfer does nothing even when the graph is absent (the no-op is checked before the source).
            update(db, &format!("{op} :absent TO :absent"));
            update(db, &format!("{op} DEFAULT TO DEFAULT"));
            assert_eq!(objects(db, Some(":g")), vec![lit(1)], "{op}");
        }
        assert_eq!(graphs(db), g(&["g"]), "self-transfers of an absent graph must not create it");
        // An empty source graph exists: COPY from it empties the destination.
        update(db, "CREATE GRAPH :e ; COPY :e TO :g");
        assert_eq!(objects(db, Some(":g")), NONE);
        assert_eq!(graphs(db), g(&["e", "g"]));
    });
}

#[test]
fn a_failed_operation_rolls_back_the_whole_request() {
    backends("atomic", |db| {
        update(db, "CREATE GRAPH :g");
        assert!(matches!(fails(db, "INSERT DATA { GRAPH :g { :a :p 1 } } ; CREATE GRAPH :g"), DatabaseError::GraphState(_)));
        assert_eq!(objects(db, Some(":g")), NONE);
    });
}

#[test]
fn management_survives_reopen() {
    let dir = std::env::temp_dir().join(format!("tl-graph-mgmt-reopen-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let db = Database::open(&dir).unwrap();
    update(&db, "INSERT DATA { GRAPH :g { :a :p 1 } } ; CREATE GRAPH :e ; COPY :g TO :h ; DROP GRAPH :g");
    drop(db);
    let db = Database::open(&dir).unwrap();
    assert_eq!(graphs(&db), g(&["e", "h"]));
    assert_eq!(objects(&db, Some(":h")), vec![lit(1)]);
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unresolved_load_obeys_silent_and_management_syntax_is_checked() {
    let db = Database::in_memory();
    assert!(matches!(fails(&db, "LOAD <http://example.org/data.nt>"), DatabaseError::Unsupported(_)));
    assert_eq!(update(&db, "LOAD SILENT <http://example.org/data.nt>"), 0);
    assert_eq!(update(&db, "LOAD SILENT <http://example.org/data.nt> INTO GRAPH :g"), 0);
    assert!(graphs(&db).is_empty());
    assert!(matches!(fails(&db, "CREATE :g"), DatabaseError::Syntax(_)));
    assert!(matches!(fails(&db, "DROP :g"), DatabaseError::Syntax(_)));
    assert!(matches!(fails(&db, "COPY :g :h"), DatabaseError::Syntax(_)));
}
