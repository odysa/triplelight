//! SPARQL JSON answers stream: an HTTP/1.1 client gets a chunked body, an HTTP/1.0 client the same bytes closed by the
//! connection, and both are the complete answer. Runs only when the `server` feature is built.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Enough rows to span several serialization windows (4,096 x threads x 4 single-column rows) on any machine we run on.
const ROWS: usize = 450_000;

struct Server(Child, u16, std::path::PathBuf);
impl Drop for Server {
    fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); let _ = std::fs::remove_dir_all(&self.2); }
}

fn start() -> Server {
    let dir = std::env::temp_dir().join(format!("tl-stream-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let data = dir.join("data.nt");
    let mut nt = String::with_capacity(ROWS * 40);
    for i in 0..ROWS { nt.push_str(&format!("<urn:s{i}> <urn:p> \"v{i}\" .\n")) }
    std::fs::write(&data, nt).unwrap();
    let store = dir.join("store");
    let port = { let l = TcpListener::bind("127.0.0.1:0").unwrap(); l.local_addr().unwrap().port() };
    let child = Command::new(env!("CARGO_BIN_EXE_triplelight"))
        .arg("serve-store").arg(&store).arg(port.to_string()).arg("--init").arg(&data)
        .stdout(Stdio::null()).stderr(Stdio::null())
        .env_remove("TRIPLELIGHT_UPDATE_TOKEN").env_remove("TRIPLELIGHT_HOST").spawn().unwrap();
    let server = Server(child, port, dir);
    let deadline = Instant::now() + Duration::from_secs(120);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "server did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    server
}

fn query(port: u16, version: &str, q: &str) -> Vec<u8> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(120))).unwrap();
    write!(s, "POST /sparql HTTP/{version}\r\nHost: localhost\r\nContent-Type: application/sparql-query\r\nAccept: application/sparql-results+json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{q}", q.len()).unwrap();
    let mut r = Vec::new();
    s.read_to_end(&mut r).unwrap();
    r
}

/// Splits a response into its head and body, decoding a chunked body.
fn body_of(response: &[u8]) -> (String, Vec<u8>) {
    let at = response.windows(4).position(|w| w == b"\r\n\r\n").expect("response head") + 4;
    let head = String::from_utf8_lossy(&response[..at]).to_string();
    let raw = &response[at..];
    if !head.to_ascii_lowercase().contains("transfer-encoding: chunked") { return (head, raw.to_vec()) }
    let (mut out, mut rest) = (Vec::new(), raw);
    loop {
        let eol = rest.windows(2).position(|w| w == b"\r\n").expect("chunk size line");
        let size = usize::from_str_radix(std::str::from_utf8(&rest[..eol]).unwrap().split(';').next().unwrap().trim(), 16).unwrap();
        rest = &rest[eol + 2..];
        if size == 0 { break }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
    (head, out)
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    let (mut n, mut at) = (0, 0);
    while let Some(i) = haystack[at..].windows(needle.len()).position(|w| w == needle) { n += 1; at += i + needle.len() }
    n
}

#[test]
fn a_large_json_answer_is_chunked_complete_and_identical_over_http_1_0() {
    let server = start();
    let q = "SELECT ?s WHERE { ?s <urn:p> ?o }";
    let (head, chunked) = body_of(&query(server.1, "1.1", q));
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(head.to_ascii_lowercase().contains("transfer-encoding: chunked"), "{head}");
    assert!(head.to_ascii_lowercase().contains("application/sparql-results+json"), "{head}");
    assert!(chunked.starts_with(br#"{"head":{"vars":["s"]},"results":{"bindings":[{"s":"#));
    assert!(chunked.ends_with(b"]}}"));
    assert_eq!(count(&chunked, br#"{"s":{"type":"uri""#), ROWS);
    // A client that cannot take chunks gets the same body, closed by the connection.
    let (head10, identity) = body_of(&query(server.1, "1.0", q));
    assert!(head10.starts_with("HTTP/1.1 200") || head10.starts_with("HTTP/1.0 200"), "{head10}");
    assert!(!head10.to_ascii_lowercase().contains("transfer-encoding: chunked"), "{head10}");
    assert_eq!(identity, chunked);
}

#[test]
fn errors_before_the_first_byte_keep_their_status() {
    let server = start();
    let (head, _) = body_of(&query(server.1, "1.1", "SELECT ?s WHERE {"));
    assert!(head.starts_with("HTTP/1.1 400"), "{head}");
    let (head, body) = body_of(&query(server.1, "1.1", "SELECT ?s WHERE { ?s <urn:absent> ?o }"));
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(String::from_utf8(body).unwrap(), r#"{"head":{"vars":["s"]},"results":{"bindings":[]}}"#);
}
