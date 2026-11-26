//! `serve-store --read-only`: several processes serve one store under a shared lock, nothing under the store directory changes, a writer is
//! refused while readers are up, and updates are refused. Runs only when the `server` feature is built.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use super::server_spawn::spawn_server;

const ROWS: usize = 2_000;
/// How long one attempt to start a server may take (`spawn_server`).
const UP: Duration = Duration::from_secs(60);

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
}

fn free_port() -> u16 { TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port() }

fn serve(store: &Path, port: u16, extra: &[&Path], flags: &[&str]) -> Child {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_triplelight"));
    cmd.arg("serve-store").arg(store).arg(port.to_string()).args(flags);
    for p in extra { cmd.arg(p); }
    cmd.stdout(Stdio::null()).stderr(Stdio::piped()).env_remove("TRIPLELIGHT_UPDATE_TOKEN").env_remove("TRIPLELIGHT_HOST").spawn().unwrap()
}

fn post(port: u16, path: &str, ctype: &str, body: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    write!(s, "POST {path} HTTP/1.0\r\nHost: localhost\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut r = String::new();
    s.read_to_string(&mut r).unwrap();
    r
}

fn count(port: u16) -> String {
    post(port, "/sparql", "application/sparql-query", "SELECT (COUNT(*) AS ?n) WHERE { ?s <urn:p> ?o }")
}

/// Every file under `dir` with its size and modification time: what "nothing changed" means.
fn listing(dir: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    let mut out = Vec::new();
    fn walk(d: &Path, out: &mut Vec<(PathBuf, u64, std::time::SystemTime)>) {
        for e in std::fs::read_dir(d).unwrap() {
            let e = e.unwrap();
            let m = e.metadata().unwrap();
            out.push((e.path(), m.len(), m.modified().unwrap()));
            if m.is_dir() { walk(&e.path(), out) }
        }
    }
    walk(dir, &mut out);
    out.sort();
    out
}

#[test]
fn several_read_only_servers_share_one_store_and_change_nothing() {
    let root = std::env::temp_dir().join(format!("tl-readonly-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let data = root.join("data.nt");
    std::fs::write(&data, (0..ROWS).map(|i| format!("<urn:s{i}> <urn:p> \"v{i}\" .\n")).collect::<String>()).unwrap();
    let store = root.join("store");

    // A writer builds the store (with a base on disk), answers once, and goes away.
    let (wport, w) = spawn_server(&store, &["--disk", "--init", data.to_str().unwrap()], &[], UP);
    let writer = Server(w);
    assert!(count(wport).contains(&format!("\"{ROWS}\"")), "the writer sees all rows");
    drop(writer);
    std::thread::sleep(Duration::from_millis(300));
    let before = listing(&store);

    // Two read-only servers at once, each answering from the same store.
    let (p1, c1) = spawn_server(&store, &["--disk", "--read-only"], &[], UP);
    let r1 = Server(c1);
    let (p2, c2) = spawn_server(&store, &["--disk", "--read-only"], &[], UP);
    let r2 = Server(c2);
    for p in [p1, p2] { assert!(count(p).contains(&format!("\"{ROWS}\"")), "read-only server on {p} answers with every row"); }

    // A writer is refused while readers are open, naming the reason.
    let mut w = serve(&store, free_port(), &[], &["--disk"]);
    let status = w.wait().unwrap();
    let mut why = String::new();
    w.stderr.take().unwrap().read_to_string(&mut why).unwrap();
    assert!(!status.success() && why.contains("already open"), "a writer must be refused while readers are open: {why}");

    // Updates are refused (403), and the answer is unchanged.
    let upd = post(p1, "/update", "application/sparql-update", "INSERT DATA { <urn:x> <urn:p> \"y\" }");
    assert!(upd.contains(" 403 "), "{upd}");
    assert!(count(p1).contains(&format!("\"{ROWS}\"")));

    drop((r1, r2));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(listing(&store), before, "serving read-only must not create, change or remove anything under the store directory");

    // The store is whole: a writer opens it again and sees the same rows.
    let (wport, w) = spawn_server(&store, &["--disk"], &[], UP);
    let writer = Server(w);
    assert!(count(wport).contains(&format!("\"{ROWS}\"")));
    drop(writer);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn read_only_refuses_init_and_a_store_that_does_not_exist() {
    let root = std::env::temp_dir().join(format!("tl-readonly-refuse-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let data = root.join("d.nt");
    std::fs::write(&data, "<urn:a> <urn:p> \"1\" .\n").unwrap();
    let mut c = serve(&root.join("s"), free_port(), &[], &["--read-only", "--init", data.to_str().unwrap()]);
    assert_eq!(c.wait().unwrap().code(), Some(2), "--read-only with --init is a usage error");
    let mut c = serve(&root.join("missing"), free_port(), &[], &["--read-only"]);
    let status = c.wait().unwrap();
    let mut why = String::new();
    c.stderr.take().unwrap().read_to_string(&mut why).unwrap();
    assert!(!status.success() && why.contains("not a store"), "{why}");
    assert!(!root.join("missing").exists() && !root.join("s").exists(), "a read-only server creates nothing");
    let _ = std::fs::remove_dir_all(&root);
}
