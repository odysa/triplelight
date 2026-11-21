//! Loading N-Quads and TriG files into named graphs (task #36).
use triplelight::embedded::{Database, DatabaseError};
use triplelight::exec::QueryResult;

fn rows(db: &Database, q: &str) -> Vec<Vec<String>> {
    match db.query(&format!("PREFIX : <http://ex/> {q}")).unwrap() {
        QueryResult::Solutions { rows, .. } => {
            let mut v: Vec<Vec<String>> = rows.into_iter().map(|r| r.into_iter().map(|t| t.map(|t| t.to_string()).unwrap_or("-".into())).collect()).collect();
            v.sort();
            v
        }
        r => panic!("{r:?}"),
    }
}

fn file(dir: &std::path::Path, name: &str, text: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

const NQ: &str = "<http://ex/a> <http://ex/p> \"1\" <http://ex/g> .\n\
                  <http://ex/a> <http://ex/p> \"2\" .\n\
                  _:b <http://ex/p> \"3\" <http://ex/g> .\n\
                  _:b <http://ex/q> \"4\" <http://ex/h> .\n";
const TRIG: &str = "@prefix : <http://ex/> .\n\
                    :a :p \"2\" .\n\
                    :g { :a :p \"1\" . _:b :p \"3\" }\n\
                    GRAPH :h { _:b :q \"4\" }\n";

fn check(db: &Database) {
    assert_eq!(rows(db, "SELECT ?g { GRAPH ?g { } }"), vec![vec!["<http://ex/g>".to_string()], vec!["<http://ex/h>".to_string()]]);
    assert_eq!(rows(db, "SELECT ?o { :a :p ?o }"), vec![vec!["\"2\"".to_string()]]);
    assert_eq!(rows(db, "SELECT ?o { GRAPH :g { ?s :p ?o } }"), vec![vec!["\"1\"".to_string()], vec!["\"3\"".to_string()]]);
    // One blank-node label within a file is one node, across its graphs.
    assert_eq!(rows(db, "SELECT ?o { GRAPH :g { ?b :p \"3\" } GRAPH :h { ?b :q ?o } }"), vec![vec!["\"4\"".to_string()]]);
}

#[test]
fn nquads_and_trig_load_into_named_graphs_on_both_backends() {
    let dir = std::env::temp_dir().join(format!("tl-quad-load-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let nq = file(&dir, "data.nq", NQ);
    let trig = file(&dir, "data.trig", TRIG);
    for source in [&nq, &trig] {
        let db = Database::in_memory();
        assert_eq!(db.load(source).unwrap(), 4);
        check(&db);
        let store = dir.join(format!("store-{}", source.extension().unwrap().to_str().unwrap()));
        let db = Database::open(&store).unwrap();
        assert_eq!(db.load(source).unwrap(), 4);
        check(&db);
        drop(db);
        let db = Database::open(&store).unwrap();
        check(&db);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn blank_nodes_are_scoped_to_each_loaded_file() {
    let dir = std::env::temp_dir().join(format!("tl-quad-scope-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let nq = file(&dir, "data.nq", "_:b <http://ex/p> \"1\" <http://ex/g> .\n");
    let db = Database::in_memory();
    db.load(&nq).unwrap();
    db.load(&nq).unwrap();
    assert_eq!(rows(&db, "SELECT ?b { GRAPH :g { ?b :p \"1\" } }").len(), 2);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bad_input_leaves_the_database_unchanged() {
    let dir = std::env::temp_dir().join(format!("tl-quad-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let syntax = file(&dir, "bad.nq", "<http://ex/a> <http://ex/p> \"1\" <http://ex/g> .\n<http://ex/a> <http://ex/p> .\n");
    let blank_graph = file(&dir, "blank.nq", "<http://ex/a> <http://ex/p> \"1\" <http://ex/g> .\n<http://ex/a> <http://ex/p> \"1\" _:g .\n");
    for db in [Database::in_memory(), Database::open(dir.join("store")).unwrap()] {
        assert!(matches!(db.load(&syntax), Err(DatabaseError::Syntax(_))));
        assert!(matches!(db.load(&blank_graph), Err(DatabaseError::Unsupported(_))));
        assert_eq!(rows(&db, "SELECT ?g { GRAPH ?g { } }"), Vec::<Vec<String>>::new());
    }
    std::fs::remove_dir_all(dir).unwrap();
}
