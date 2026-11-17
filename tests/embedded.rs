#![cfg(unix)]
use rdfdb::embedded::{Database, DatabaseOptions};
use rdfdb::exec::{QueryErrorKind, QueryResult};
use std::path::PathBuf;

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rdfdb-embedded-{name}-{}", std::process::id()))
}
fn ask(db: &Database, query: &str) -> bool {
    match db.query(query).unwrap() { QueryResult::Boolean(b) => b, x => panic!("expected ASK: {x:?}") }
}
const ASK: &str = "ASK { <http://example/s> <http://example/p> <http://example/o> }";
const INSERT: &str = "INSERT DATA { <http://example/s> <http://example/p> <http://example/o> }";
const DELETE: &str = "DELETE DATA { <http://example/s> <http://example/p> <http://example/o> }";

#[test]
fn embedded_update_snapshot_and_reopen() {
    let dir = temp("restart");
    let _ = std::fs::remove_dir_all(&dir);
    let db = Database::open(&dir).unwrap();
    assert!(!ask(&db, ASK));
    assert_eq!(db.update(INSERT).unwrap(), 1);
    let old = db.read();
    assert_eq!(db.update(DELETE).unwrap(), 1);
    assert!(!ask(&db, ASK));
    assert!(matches!(old.query(ASK).unwrap(), QueryResult::Boolean(true)));
    db.update(INSERT).unwrap();
    db.compact().unwrap();
    drop(old); drop(db);
    let reopened = Database::open(&dir).unwrap();
    assert!(ask(&reopened, ASK));
    assert_eq!(reopened.query("not a query").unwrap_err().kind, QueryErrorKind::Syntax);
    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn exclusive_open_and_recovery() {
    let dir = temp("lock");
    let _ = std::fs::remove_dir_all(&dir);
    let db = Database::open(&dir).unwrap();
    assert!(Database::open(&dir).is_err());
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lock_child", "--nocapture"])
        .env("RDFDB_LOCK_TEST_DIR", &dir).status().unwrap();
    assert!(child.success());
    db.update(INSERT).unwrap();
    drop(db);
    let reopened = Database::open(&dir).unwrap();
    assert!(ask(&reopened, ASK));
    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn lock_child() {
    if let Some(dir) = std::env::var_os("RDFDB_LOCK_TEST_DIR") {
        assert!(Database::open(PathBuf::from(dir)).is_err());
    }
}

#[test]
fn zero_workers_rejected_before_store_creation() {
    let dir = temp("workers");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(Database::open_with(&dir, DatabaseOptions { initial: None, import_workers: 0 }).is_err());
    assert!(!dir.exists());
}
