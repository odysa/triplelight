//! Starting `serve-store` for a test: shared by every server test module, since they all pick ports in parallel.
//! A retry reruns the same command line: `--init` on a store that a failed attempt already built is a no-op (the store
//! has a CURRENT), so a retried server opens what the first attempt loaded.
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Starts `serve-store <dir> <port> <flags>` on a free port and waits up to `timeout` per attempt until it serves. The
/// port is picked and released before the server binds it, and every server test module picks ports in parallel, so
/// another test can take it in between: this server then exits (it says why on stderr) while the other test's server
/// answers on the port, and a request sent there reaches the wrong server, which is reset when that
/// test ends (#128: a "connection reset" before any HTTP status). A connect that succeeds does not show whose
/// server it reached, so the server's own "serving SPARQL at ...:<port>" line on stderr is the signal, and a child
/// that exits first is retried on a fresh port, a few times.
pub fn spawn_server(dir: &Path, flags: &[&str], env: &[(&str, &str)], timeout: Duration) -> (u16, Child) {
    use std::io::BufRead;
    (0..4).find_map(|attempt| {
        let port = { let l = TcpListener::bind("127.0.0.1:0").unwrap(); l.local_addr().unwrap().port() };
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_triplelight"));
        cmd.arg("serve-store").arg(dir).arg(port.to_string()).args(flags).stdout(Stdio::null()).stderr(Stdio::piped())
            .env_remove("TRIPLELIGHT_UPDATE_TOKEN").env_remove("TRIPLELIGHT_HOST");
        for (k, v) in env { cmd.env(k, v); }
        let mut child = cmd.spawn().unwrap();
        // stderr is drained as it is written (a server that logs more than a pipe buffer never blocks on it), and
        // the serving line is reported as soon as it appears.
        let stderr = child.stderr.take().unwrap();
        let (ready, serving) = std::sync::mpsc::channel();
        let banner = format!("serving SPARQL at http://127.0.0.1:{port}/");
        let log = std::thread::spawn(move || {
            let mut seen = String::new();
            for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
                if line.contains(&banner) { let _ = ready.send(()); }
                seen.push_str(&line);
                seen.push('\n');
            }
            seen
        });
        match serving.recv_timeout(timeout) {
            Ok(()) if child.try_wait().unwrap().is_none() => return Some((port, child)),
            _ => {}
        }
        let exited = child.try_wait().unwrap();
        let _ = child.kill();
        let _ = child.wait();
        let why = log.join().unwrap_or_default();
        assert!(attempt < 3, "server did not start after {} attempts (exited: {exited:?}): {why}", attempt + 1);
        None
    }).unwrap()
}
