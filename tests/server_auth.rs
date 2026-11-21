//! `serve-store` update authorization: read-only mode, bearer token, and refusal to expose open updates beyond
//! loopback. Runs only when the `server` feature is built.
#![cfg(all(unix, feature = "server"))]
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Each test picks a free port, releases it and starts a server on it; tests in this file run one at a time so a
/// parallel test cannot take that port in between (which would send requests to the wrong server).
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Server(Child, u16, std::path::PathBuf, #[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
impl Drop for Server {
    fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); let _ = std::fs::remove_dir_all(&self.2); }
}

fn start(name: &str, flags: &[&str], token: Option<&str>) -> Server {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("tl-auth-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let port = { let l = TcpListener::bind("127.0.0.1:0").unwrap(); l.local_addr().unwrap().port() };
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_triplelight"));
    cmd.arg("serve-store").arg(&dir).arg(port.to_string()).args(flags).stdout(Stdio::null()).stderr(Stdio::null())
        .env_remove("TRIPLELIGHT_UPDATE_TOKEN").env_remove("TRIPLELIGHT_HOST");
    if let Some(t) = token { cmd.env("TRIPLELIGHT_UPDATE_TOKEN", t); }
    let server = Server(cmd.spawn().unwrap(), port, dir, guard);
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "server did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    server
}

fn post(port: u16, path: &str, ctype: &str, body: &str, auth: Option<&str>) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let auth = auth.map(|a| format!("Authorization: {a}\r\n")).unwrap_or_default();
    write!(s, "POST {path} HTTP/1.0\r\nHost: localhost\r\nContent-Type: {ctype}\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut r = String::new();
    s.read_to_string(&mut r).unwrap();
    r
}

/// Rejected requests below send no body: the server answers before reading one, and closing a socket with unread
/// data can make the kernel reset it before the client reads the 401/403. That is a client-side effect of prompt
/// rejection (which `rejected_update_is_answered_without_reading_the_body` relies on), not what these tests check.
const UPDATE: &str = "INSERT DATA { <urn:s> <urn:p> \"v\" }";
const ASK: &str = "ASK { <urn:s> <urn:p> \"v\" }";

fn post_with(port: u16, extra: &str, body: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(s, "POST /update HTTP/1.0\r\nHost: localhost\r\nContent-Type: application/x-www-form-urlencoded\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut r = String::new();
    s.read_to_string(&mut r).unwrap();
    r
}

#[test]
fn updates_are_off_by_default_even_on_loopback() {
    let s = start("default", &[], None);
    assert!(post(s.1, "/update", "application/sparql-update", "", None).contains(" 403 "));
    assert!(post(s.1, "/sparql", "application/sparql-query", ASK, None).contains("false"));
}

#[test]
fn explicit_open_updates_refuse_browser_origins() {
    let s = start("open", &["--allow-unauthenticated-updates"], None);
    // A cross-origin HTML form POST from a web page carries an Origin header.
    let form = "update=INSERT%20DATA%20%7B%20%3Curn%3As%3E%20%3Curn%3Ap%3E%20%22v%22%20%7D";
    assert!(post_with(s.1, "Origin: https://evil.example\r\n", "").contains(" 403 "));
    assert!(post(s.1, "/sparql", "application/sparql-query", ASK, None).contains("false"));
    assert!(post_with(s.1, "", form).contains(" 200 "));
    assert!(post(s.1, "/sparql", "application/sparql-query", ASK, None).contains("true"));
}

#[test]
fn read_only_rejects_updates_but_answers_queries() {
    let s = start("ro", &["--read-only"], None);
    assert!(post(s.1, "/update", "application/sparql-update", "", None).contains(" 403 "));
    assert!(post(s.1, "/sparql", "application/sparql-query", ASK, None).contains("false"));
}

#[test]
fn token_is_required_and_checked() {
    let s = start("token", &[], Some("s3cret"));
    assert!(post(s.1, "/update", "application/sparql-update", "", None).contains(" 401 "));
    assert!(post(s.1, "/update", "application/sparql-update", "", Some("Bearer wrong")).contains(" 401 "));
    assert!(post(s.1, "/sparql", "application/sparql-query", ASK, None).contains("false"));
    assert!(post(s.1, "/update", "application/sparql-update", UPDATE, Some("Bearer s3cret")).contains(" 200 "));
    assert!(post(s.1, "/sparql", "application/sparql-query", ASK, None).contains("true"));
}

#[test]
fn refuses_open_updates_on_a_public_address() {
    let dir = std::env::temp_dir().join(format!("tl-auth-public-{}", std::process::id()));
    let out = Command::new(env!("CARGO_BIN_EXE_triplelight")).arg("serve-store").arg(&dir).arg("0")
        .arg("--allow-unauthenticated-updates").env_remove("TRIPLELIGHT_UPDATE_TOKEN").env("TRIPLELIGHT_HOST", "0.0.0.0").output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("TRIPLELIGHT_UPDATE_TOKEN"));
}

#[test]
fn rejected_update_is_answered_without_reading_the_body() {
    let s = start("nobody", &[], Some("s3cret"));
    let mut c = TcpStream::connect(("127.0.0.1", s.1)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    // Declares a 1 GB body but sends none: the 401 must still arrive.
    write!(c, "POST /update HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/sparql-update\r\nContent-Length: 1000000000\r\n\r\n").unwrap();
    let mut buf = [0u8; 64];
    let n = c.read(&mut buf).expect("response before the body was sent");
    assert!(String::from_utf8_lossy(&buf[..n]).contains(" 401 "));
}

#[test]
fn a_client_withholding_its_body_does_not_block_other_requests() {
    let s = start("slow", &[], Some("s3cret"));
    // Declares a small body, sends none, and keeps the connection open.
    let mut slow = TcpStream::connect(("127.0.0.1", s.1)).unwrap();
    write!(slow, "POST /update HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/sparql-update\r\nContent-Length: 65536\r\n\r\n").unwrap();
    slow.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut buf = [0u8; 64];
    let n = slow.read(&mut buf).expect("the rejection is sent without waiting for the body");
    assert!(String::from_utf8_lossy(&buf[..n]).contains(" 401 "));
    // A legitimate query on another connection completes while the slow one is still open.
    assert!(post(s.1, "/sparql", "application/sparql-query", ASK, None).contains("false"));
    drop(slow);
}

#[test]
fn an_unauthorized_update_with_a_full_body_is_not_applied() {
    let s = start("unauth-full", &[], Some("s3cret"));
    // Complete, valid update without credentials. The server may answer 401 or, because it rejects without
    // reading the body, the connection may be reset; either way nothing may be inserted.
    let mut c = TcpStream::connect(("127.0.0.1", s.1)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(c, "POST /update HTTP/1.0\r\nHost: localhost\r\nContent-Type: application/sparql-update\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{UPDATE}", UPDATE.len()).unwrap();
    let mut r = String::new();
    match c.read_to_string(&mut r) {
        Ok(_) => assert!(r.contains(" 401 "), "{r}"),
        Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::ConnectionReset, "{e}"),
    }
    assert!(post(s.1, "/sparql", "application/sparql-query", ASK, None).contains("false"));
    // The same update with the token is applied (so the check above is not vacuous).
    assert!(post(s.1, "/update", "application/sparql-update", UPDATE, Some("Bearer s3cret")).contains(" 200 "));
    assert!(post(s.1, "/sparql", "application/sparql-query", ASK, None).contains("true"));
}

/// A graph-management update that names a graph in the wrong state is the client's error (400), not the server's.
#[test]
fn graph_state_errors_are_client_errors() {
    let s = start("graph-state", &["--allow-unauthenticated-updates"], None);
    let update = |body: &str| post(s.1, "/update", "application/sparql-update", body, None);
    assert!(update("CREATE GRAPH <urn:g>").contains(" 200 "));
    assert!(update("CREATE GRAPH <urn:g>").contains(" 400 "));
    assert!(update("DROP GRAPH <urn:absent>").contains(" 400 "));
    assert!(update("COPY <urn:absent> TO <urn:g>").contains(" 400 "));
    assert!(update("DROP SILENT GRAPH <urn:absent>").contains(" 200 "));
}
