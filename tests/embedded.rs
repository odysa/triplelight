#![cfg(unix)]
use triplelight::embedded::{Database, DatabaseError, DatabaseOptions, QueryWriteError};
use triplelight::exec::QueryResult;
use std::path::PathBuf;

fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("triplelight-embedded-{name}-{}", std::process::id()))
}
fn ask(db: &Database, query: &str) -> bool {
    match db.query(query).unwrap() { QueryResult::Boolean(b) => b, x => panic!("expected ASK: {x:?}") }
}
/// Spawning a child process can briefly duplicate this process's open descriptors (including another test's
/// directory lock) until the child execs; tests asserting that a closed directory reopens at once hold this.
static SPAWN: std::sync::Mutex<()> = std::sync::Mutex::new(());
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
    let _serial = SPAWN.lock().unwrap_or_else(|e| e.into_inner());
    let dir = temp("lock");
    let _ = std::fs::remove_dir_all(&dir);
    let db = Database::open(&dir).unwrap();
    assert!(Database::open(&dir).is_err());
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lock_child", "--nocapture"])
        .env("TRIPLELIGHT_LOCK_TEST_DIR", &dir).status().unwrap();
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
    if let Some(dir) = std::env::var_os("TRIPLELIGHT_LOCK_TEST_DIR") {
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

#[test]
fn memory_database_snapshot_update_and_bulk_load() {
    use triplelight::embedded::Database;
    let db = Database::in_memory();
    let old = db.read();
    assert_eq!(db.update("INSERT DATA { <urn:a> <urn:p> <urn:b> }").unwrap(), 1);
    assert!(old.query_json("ASK { <urn:a> <urn:p> <urn:b> }").unwrap().contains("false"));
    assert!(db.query_json("ASK { <urn:a> <urn:p> <urn:b> }").unwrap().contains("true"));
    assert_eq!(db.update("INSERT DATA { <urn:a> <urn:p> <urn:b> }").unwrap(), 0);
    let copy = db.clone();
    copy.update("DELETE DATA { <urn:a> <urn:p> <urn:b> }").unwrap();
    assert!(db.query_json("ASK { <urn:a> <urn:p> <urn:b> }").unwrap().contains("false"));
}

#[test]
fn memory_bulk_import_and_blank_node_scope() {
    let path = temp("memory-import").with_extension("nt");
    std::fs::write(&path, "_:a <urn:p> _:a .\n_:a <urn:p> _:a .\n").unwrap();
    let db = Database::in_memory();
    assert_eq!(db.load(&path).unwrap(), 1);
    let before = db.read();
    assert_eq!(db.load(&path).unwrap(), 1);
    let result = db.query_json("SELECT (COUNT(*) AS ?n) WHERE { ?s <urn:p> ?s }").unwrap();
    assert!(result.contains("\"2\""), "{result}");
    let result = before.query_json("SELECT (COUNT(*) AS ?n) WHERE { ?s <urn:p> ?s }").unwrap();
    assert!(result.contains("\"1\""), "{result}");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn memory_concurrent_updates_are_serializable() {
    let db = Database::in_memory();
    let threads: Vec<_> = (0..8).map(|i| {
        let db = db.clone();
        std::thread::spawn(move || db.update(&format!("INSERT DATA {{ <urn:s{i}> <urn:p> <urn:o> }}")).unwrap())
    }).collect();
    for thread in threads { assert_eq!(thread.join().unwrap(), 1); }
    let result = db.query_json("SELECT (COUNT(*) AS ?n) WHERE { ?s <urn:p> <urn:o> }").unwrap();
    assert!(result.contains("\"8\""), "{result}");
}

#[test]
fn memory_compaction_retains_old_snapshot() {
    let db = Database::in_memory();
    db.update(INSERT).unwrap();
    let old = db.read();
    db.update(DELETE).unwrap();
    db.compact().unwrap();
    assert!(!ask(&db, ASK));
    assert!(matches!(old.query(ASK).unwrap(), QueryResult::Boolean(true)));
    for _ in 0..1000 {
        if db.stats_ready() { return; }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("statistics not available after compaction");
}

#[test]
fn disk_import_scopes_blank_nodes_per_document() {
    let dir = temp("disk-blank-scopes");
    let path = temp("disk-blank-scopes-input").with_extension("nt");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::write(&path, "_:a <urn:p> _:a .\n").unwrap();
    let db = Database::open(&dir).unwrap();
    assert_eq!(db.load(&path).unwrap(), 1);
    assert_eq!(db.load(&path).unwrap(), 1);
    let result = db.query_json("SELECT (COUNT(*) AS ?n) WHERE { ?s <urn:p> ?s }").unwrap();
    assert!(result.contains("\"2\""), "{result}");
    drop(db);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn memory_streamed_import_is_atomic_on_late_parse_error() {
    let path = temp("memory-late-error").with_extension("nt");
    std::fs::write(&path, "<urn:a> <urn:p> <urn:b> .\nnot a triple\n").unwrap();
    let db = Database::in_memory();
    assert!(db.load(&path).is_err());
    assert!(!ask(&db, "ASK { <urn:a> <urn:p> <urn:b> }"));
    std::fs::write(&path, "<urn:a> <urn:p> <urn:b> .\n").unwrap();
    assert_eq!(db.load(&path).unwrap(), 1);
    assert!(ask(&db, "ASK { <urn:a> <urn:p> <urn:b> }"));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn memory_streamed_turtle_load_is_atomic_and_document_scoped() {
    let path = temp("memory-turtle").with_extension("ttl");
    let db = Database::in_memory();
    std::fs::write(&path, "@prefix ex: <urn:example:> . _:a ex:p _:a . ex:s ex:p").unwrap();
    assert!(db.load(&path).is_err());
    assert!(!ask(&db, "ASK { ?s <urn:example:p> ?s }"));
    std::fs::write(&path, "@prefix ex: <urn:example:> . _:a ex:p _:a .").unwrap();
    assert_eq!(db.load(&path).unwrap(), 1);
    assert_eq!(db.load(&path).unwrap(), 1);
    let count = db.query_json("SELECT (COUNT(*) AS ?n) WHERE { ?s <urn:example:p> ?s }").unwrap();
    assert!(count.contains("\"2\""), "{count}");
    std::fs::remove_file(path).unwrap();
}

// ---------- persisted planner statistics (task #34) ----------
use triplelight::embedded::StatsOrigin;

fn seeded(name: &str) -> PathBuf {
    let dir = temp(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let nt = dir.with_extension("nt");
    let mut text = String::new();
    for i in 0..500 { text.push_str(&format!("<http://example/s{i}> <http://example/p{}> \"{i}\" .\n", i % 7)); }
    std::fs::write(&nt, text).unwrap();
    let db = Database::open_with(dir.join("db"), DatabaseOptions { initial: Some(nt), ..Default::default() }).unwrap();
    wait_stats(&db);
    dir
}
fn wait_stats(db: &Database) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !(db.stats_ready() && db.stats_origin().is_some()) && std::time::Instant::now() < deadline { std::thread::sleep(std::time::Duration::from_millis(2)); }
    assert!(db.stats_ready() && db.stats_origin().is_some(), "statistics not ready");
}
fn stats_files(db_dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(db_dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("stats-")).collect();
    v.sort();
    v
}
const COUNT_P3: &str = "SELECT (COUNT(*) AS ?n) WHERE { ?s <http://example/p3> ?o }";

#[test]
fn persisted_stats_are_reused_on_reopen() {
    let dir = seeded("stats-reuse");
    let db_dir = dir.join("db");
    assert_eq!(stats_files(&db_dir), ["stats-0.v1"]);
    let db = Database::open(&db_dir).unwrap();
    // Loaded synchronously at open: ready immediately, no scan.
    assert!(db.stats_ready());
    assert!(matches!(db.stats_origin(), Some(StatsOrigin::Persisted { bytes, .. }) if bytes > 0));
    assert!(db.query_json(COUNT_P3).unwrap().contains("\"71\""));
    drop(db);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn unusable_stats_files_fall_back_to_a_scan() {
    let dir = seeded("stats-corrupt");
    let db_dir = dir.join("db");
    let file = db_dir.join("stats-0.v1");
    let good = std::fs::read(&file).unwrap();
    let flip_payload = { let mut b = good.clone(); let i = b.len() - 1; b[i] ^= 1; b };
    let wrong_generation = { let mut b = good.clone(); b[8] ^= 1; b };
    for (what, bytes) in [("truncated", good[..good.len() / 2].to_vec()), ("payload bit flip", flip_payload),
                          ("wrong generation", wrong_generation), ("empty", Vec::new())] {
        std::fs::write(&file, &bytes).unwrap();
        let db = Database::open(&db_dir).unwrap();
        wait_stats(&db);
        assert!(matches!(db.stats_origin(), Some(StatsOrigin::Scanned { persist_ms: Some(_), .. })), "{what}: {:?}", db.stats_origin());
        assert!(db.query_json(COUNT_P3).unwrap().contains("\"71\""), "{what}");
        drop(db);
        assert_eq!(std::fs::read(&file).unwrap(), good, "{what}: rescan rewrites the same statistics");
    }
    // A MANIFEST that differs from the one the file was bound to: the file no longer applies.
    let manifest = db_dir.join("base-0").join("MANIFEST");
    let original = std::fs::read(&manifest).unwrap();
    let bound = std::fs::read(&file).unwrap();
    let mut forged = bound.clone(); forged[16] ^= 1;
    std::fs::write(&file, &forged).unwrap();
    let db = Database::open(&db_dir).unwrap();
    wait_stats(&db);
    assert!(matches!(db.stats_origin(), Some(StatsOrigin::Scanned { .. })));
    drop(db);
    assert_eq!(std::fs::read(&manifest).unwrap(), original);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn compaction_replaces_the_stats_file_and_stale_scans_are_dropped() {
    let dir = seeded("stats-compact");
    let db_dir = dir.join("db");
    // Reopen without the file so a scan of base 0 runs, then compact at once: that scan must not be installed or
    // persisted for the new base.
    std::fs::remove_file(db_dir.join("stats-0.v1")).unwrap();
    let db = Database::open(&db_dir).unwrap();
    db.update(INSERT).unwrap();
    db.compact().unwrap();
    wait_stats(&db);
    assert!(db.query_json(COUNT_P3).unwrap().contains("\"71\""));
    // Allow the stale base-0 scan to finish; it must not resurrect stats-0.v1.
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(stats_files(&db_dir), ["stats-1.v1"]);
    drop(db);
    let db = Database::open(&db_dir).unwrap();
    assert!(matches!(db.stats_origin(), Some(StatsOrigin::Persisted { .. })));
    assert!(ask(&db, ASK));
    drop(db);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn closing_during_a_scan_releases_the_directory_and_writes_nothing_late() {
    let dir = seeded("stats-close");
    let db_dir = dir.join("db");
    std::fs::remove_file(db_dir.join("stats-0.v1")).unwrap();
    let _serial = SPAWN.lock().unwrap_or_else(|e| e.into_inner());
    for _ in 0..20 {
        let db = Database::open(&db_dir).unwrap();
        drop(db); // scan still running: the directory must be free immediately
    }
    let db = Database::open(&db_dir).unwrap();
    wait_stats(&db);
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(stats_files(&db_dir), ["stats-0.v1"], "no temp files or foreign generations left behind");
    drop(db);
    std::fs::remove_dir_all(&dir).unwrap();
}


/// A writer that holds what it was given and refuses everything after `limit` bytes.
struct Limited { bytes: Vec<u8>, limit: usize }
impl std::io::Write for Limited {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len() + buf.len() > self.limit { return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "reader went away")) }
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

fn big_database(name: &str, rows: usize) -> (Database, PathBuf) {
    let file = temp(name).with_extension("nt");      // one file per test: tests run in parallel in one process
    std::fs::write(&file, (0..rows).map(|i| format!("<urn:s{i}> <urn:p> \"v{i}\" .\n")).collect::<String>()).unwrap();
    let db = Database::in_memory();
    db.load(&file).unwrap();
    (db, file)
}

#[test]
fn query_json_to_writer_writes_exactly_the_bytes_of_query_json() {
    let (db, file) = big_database("writer-data-1", 300_000);      // several serialization windows on any machine
    for q in ["SELECT ?s ?o WHERE { ?s <urn:p> ?o }", "SELECT ?s WHERE { ?s <urn:p> \"v7\" }", "SELECT ?s WHERE { ?s <urn:absent> ?o }", "ASK { <urn:s1> <urn:p> ?o }", "SELECT (COUNT(*) AS ?n) WHERE { ?s <urn:p> ?o }"] {
        let mut out = Vec::new();
        db.query_json_to_writer(q, &mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), db.query_json(q).unwrap(), "{q}");
        // the same through a pinned snapshot
        let mut out = Vec::new();
        db.read().query_json_to_writer(q, &mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), db.read().query_json(q).unwrap(), "{q} (snapshot)");
    }
    let _ = std::fs::remove_file(file);
}

#[test]
fn a_writer_that_fails_partway_leaves_an_invalid_prefix_and_an_output_error() {
    let (db, file) = big_database("writer-data-2", 300_000);
    let q = "SELECT ?s ?o WHERE { ?s <urn:p> ?o }";
    let whole = db.query_json(q).unwrap();
    let mut out = Limited { bytes: Vec::new(), limit: 1 << 20 };
    match db.query_json_to_writer(q, &mut out) {
        Err(QueryWriteError::Output(e)) => assert_eq!(e.kind(), std::io::ErrorKind::BrokenPipe),
        other => panic!("expected an output error, got {other:?}"),
    }
    assert!(!out.bytes.is_empty() && out.bytes.len() <= 1 << 20, "some of the answer was written before the writer failed");
    assert!(whole.as_bytes().starts_with(&out.bytes), "what was written is a prefix of the answer");
    assert!(!out.bytes.windows(3).any(|w| w == b"]}}"), "the closing braces were never written: the body is not valid JSON");
    let _ = std::fs::remove_file(file);
}

#[test]
fn errors_before_the_first_byte_leave_the_writer_untouched() {
    let db = Database::in_memory();
    for (q, want_unsupported) in [("SELECT ?s WHERE {", false), ("CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o }", true)] {
        let mut out = Vec::new();
        match db.query_json_to_writer(q, &mut out) {
            Err(QueryWriteError::Query(DatabaseError::Syntax(_))) if !want_unsupported => {}
            Err(QueryWriteError::Query(DatabaseError::Unsupported(_))) if want_unsupported => {}
            other => panic!("{q}: {other:?}"),
        }
        assert!(out.is_empty(), "{q}: nothing is written when the query fails first");
    }
}

#[test]
fn query_json_to_writer_honors_limits_before_writing() {
    let (db, file) = big_database("writer-data-3", 2_000);
    let interrupt = triplelight::exec::Interrupt::new();
    interrupt.cancel();
    let mut out = Vec::new();
    let limits = triplelight::exec::QueryLimits { interrupt: Some(&interrupt), memory: None };
    match db.query_json_to_writer_with_limits("SELECT ?s ?o WHERE { ?s <urn:p> ?o }", triplelight::exec::Entailment::Simple, limits, &mut out) {
        Err(QueryWriteError::Query(DatabaseError::Interrupted { .. })) => {}
        other => panic!("expected an interrupted query, got {other:?}"),
    }
    assert!(out.is_empty());
    let _ = std::fs::remove_file(file);
}
