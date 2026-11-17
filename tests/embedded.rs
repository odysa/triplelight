#![cfg(unix)]
use rdfdb::embedded::{Database, DatabaseError, DatabaseOptions};
use rdfdb::exec::QueryResult;
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
    assert!(matches!(reopened.query("not a query"), Err(DatabaseError::Syntax(_))));
    assert!(reopened.query_json(ASK).unwrap().contains("true"));
    assert!(matches!(Database::open(&dir), Err(DatabaseError::Locked)));
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

#[test]
fn import_is_atomic_and_clone_shares_lock() {
    let dir = temp("load");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("data.nt");
    std::fs::write(&source, "<http://example/s> <http://example/p> <http://example/o> .\ninvalid\n").unwrap();
    let db = Database::open(dir.join("db")).unwrap();
    assert!(db.load(&source).is_err());
    assert!(!ask(&db, ASK));
    std::fs::write(&source, "<http://example/s> <http://example/p> <http://example/o> .\n").unwrap();
    assert_eq!(db.load(&source).unwrap(), 1);
    let clone = db.clone();
    drop(db);
    assert!(ask(&clone, ASK));
    assert!(matches!(Database::open(dir.join("db")), Err(DatabaseError::Locked)));
    drop(clone);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn statistics_are_not_used_for_modified_or_other_generations() {
    let dir = temp("stats");
    let _ = std::fs::remove_dir_all(&dir);
    let db = Database::open(&dir).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !db.stats_ready() && std::time::Instant::now() < deadline { std::thread::yield_now(); }
    assert!(db.stats_ready());
    let old = db.read();
    db.update(INSERT).unwrap();
    assert!(!db.stats_ready());
    assert!(ask(&db, ASK));
    assert!(matches!(old.query(ASK).unwrap(), QueryResult::Boolean(false)));
    db.compact().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !db.stats_ready() && std::time::Instant::now() < deadline { std::thread::yield_now(); }
    assert!(db.stats_ready());
    assert!(ask(&db, ASK));
    assert!(matches!(old.query(ASK).unwrap(), QueryResult::Boolean(false)));
    drop(old); drop(db); std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn concurrent_load_and_delete_have_serializable_outcomes() {
    let dir = temp("mixed-writers");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Database::open(dir.join("db")).unwrap();
    for i in 0..20 {
        let source = dir.join(format!("{i}.nt"));
        let triple = format!("<http://example/s{i}> <http://example/p> <http://example/o>");
        std::fs::write(&source, format!("{triple} .\n")).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let inserted = db.clone();
        let deleted = db.clone();
        let b = barrier.clone();
        let loader = std::thread::spawn(move || { b.wait(); inserted.load(source).unwrap() });
        let b = barrier.clone();
        let pattern = triple.clone();
        let updater = std::thread::spawn(move || {
            b.wait(); deleted.update(&format!("DELETE WHERE {{ {pattern} }}")).unwrap()
        });
        assert_eq!(loader.join().unwrap(), 1);
        let deleted_count = updater.join().unwrap();
        let present = ask(&db, &format!("ASK {{ {triple} }}"));
        assert!(matches!((deleted_count, present), (0, true) | (1, false)), "delete returned {deleted_count}, present={present}");
    }
    drop(db); std::fs::remove_dir_all(dir).unwrap();
}
