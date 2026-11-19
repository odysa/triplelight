#![cfg(unix)]
use std::process::Command;
#[test]
fn disk_snapshot_cli_survives_process_restart() {
    let root=std::env::temp_dir().join(format!("triplelight-disk-cli-{}",std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let nt=root.join("input.nt");let snapshot=root.join("snapshot");
    std::fs::write(&nt,"<urn:s> <urn:p> \"hello\" .\n<urn:s2> <urn:p> \"hello\"@en .\n").unwrap();
    let exe=env!("CARGO_BIN_EXE_triplelight");
    let built=Command::new(exe).arg("build-disk").arg(&nt).arg(&snapshot).arg("--parallel-load=1").output().unwrap();
    assert!(built.status.success(),"{}",String::from_utf8_lossy(&built.stderr));
    let verify=Command::new(exe).arg("verify").arg(&snapshot).output().unwrap();
    assert!(verify.status.success(),"{}",String::from_utf8_lossy(&verify.stderr));
    let query="SELECT ?s ?o WHERE { ?s <urn:p> ?o } ORDER BY ?s";
    let memory=Command::new(exe).arg("query").arg(&nt).arg(query).arg("--parallel-load=1").output().unwrap();
    for _ in 0..2 {
        let disk=Command::new(exe).arg("query").arg(&snapshot).arg(query).arg("--disk").output().unwrap();
        assert!(disk.status.success(),"{}",String::from_utf8_lossy(&disk.stderr));
        assert_eq!(disk.stdout,memory.stdout);
    }
    let second=Command::new(exe).arg("build-disk").arg(&nt).arg(&snapshot).arg("--parallel-load=1").output().unwrap();
    assert!(!second.status.success());
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(feature = "server")]
#[test]
fn disk_server_restarts_and_answers_without_original_input() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant};
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
    }
    let root=std::env::temp_dir().join(format!("triplelight-disk-server-{}",std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let nt=root.join("input.nt");let snapshot=root.join("snapshot");
    std::fs::write(&nt,"<urn:s> <urn:p> \"persisted\" .\n").unwrap();
    let exe=env!("CARGO_BIN_EXE_triplelight");
    let out=Command::new(exe).arg("build-disk").arg(&nt).arg(&snapshot).arg("--parallel-load=1").output().unwrap();
    assert!(out.status.success(),"{}",String::from_utf8_lossy(&out.stderr));
    std::fs::remove_file(nt).unwrap();
    for _ in 0..2 {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap(); let port=listener.local_addr().unwrap().port(); drop(listener);
        let child=Command::new(exe).arg("serve").arg(&snapshot).arg(port.to_string()).arg("--disk").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let mut child=ChildGuard(child); let deadline=Instant::now()+Duration::from_secs(10);
        let mut stream=loop {
            match TcpStream::connect(("127.0.0.1",port)) {
                Ok(s)=>break s,
                Err(_) if Instant::now()<deadline => std::thread::sleep(Duration::from_millis(20)),
                Err(e)=>panic!("server failed to start: {e}, status {:?}",child.0.try_wait()),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let q="SELECT ?o WHERE { <urn:s> <urn:p> ?o }";
        write!(stream,"POST /sparql HTTP/1.0\r\nHost: localhost\r\nContent-Type: application/sparql-query\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",q.len(),q).unwrap();
        let mut response=String::new(); stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200"),"{response}");
        assert!(response.contains("persisted"),"{response}");
        drop(child);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(not(feature = "server"))]
#[test]
fn serve_without_the_server_feature_explains_how_to_enable_it() {
    let out=Command::new(env!("CARGO_BIN_EXE_triplelight")).arg("serve").arg("x.nt").output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--features server"));
}

#[test]
fn build_strategy_can_be_forced_and_matches_fast_path() {
    let root=std::env::temp_dir().join(format!("triplelight-build-strategy-{}",std::process::id()));
    std::fs::create_dir(&root).unwrap();let nt=root.join("data.nt");
    std::fs::write(&nt,"<urn:s> <urn:p> \"x\"@en .\n<urn:s> <urn:p> \"x\"@en .\n_:b <urn:p> \"7\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n").unwrap();
    let exe=env!("CARGO_BIN_EXE_triplelight");let mut answers=Vec::new();
    for (name,flags,expected) in [("fast",vec!["--build-memory-mb=1024"],"in-memory"),("ext",vec!["--build-external","--build-memory-mb=1024"],"external sort")] {
        let dir=root.join(name);let result=Command::new(exe).arg("build-disk").arg(&nt).arg(&dir).args(flags).output().unwrap();
        assert!(result.status.success(),"{}",String::from_utf8_lossy(&result.stderr));assert!(String::from_utf8_lossy(&result.stderr).contains(expected));
        let result=Command::new(exe).arg("query").arg(&dir).arg("SELECT ?o WHERE { ?s <urn:p> ?o } ORDER BY ?o").arg("--disk").output().unwrap();
        assert!(result.status.success());answers.push(result.stdout);
    }
    assert_eq!(answers[0],answers[1]);
    let bad=Command::new(exe).arg("build-disk").arg(&nt).arg(root.join("bad")).arg("--build-memory-mb=0").output().unwrap();assert!(!bad.status.success());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn disk_build_help_explains_advisory_memory_and_worker_controls() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_triplelight"))
        .args(["build-disk", "--help"]).output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("--parallel-load=N"));
    assert!(text.contains("--build-memory-mb=N"));
    assert!(text.contains("not a process RSS limit"));
}
