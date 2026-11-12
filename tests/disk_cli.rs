#![cfg(unix)]
use std::process::Command;
#[test]
fn disk_snapshot_cli_survives_process_restart() {
    let root=std::env::temp_dir().join(format!("rdfdb-disk-cli-{}",std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let nt=root.join("input.nt");let snapshot=root.join("snapshot");
    std::fs::write(&nt,"<urn:s> <urn:p> \"hello\" .\n<urn:s2> <urn:p> \"hello\"@en .\n").unwrap();
    let exe=env!("CARGO_BIN_EXE_rdfdb");
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

#[test]
fn disk_server_restarts_and_answers_without_original_input() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant};
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
    }
    let root=std::env::temp_dir().join(format!("rdfdb-disk-server-{}",std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let nt=root.join("input.nt");let snapshot=root.join("snapshot");
    std::fs::write(&nt,"<urn:s> <urn:p> \"persisted\" .\n").unwrap();
    let exe=env!("CARGO_BIN_EXE_rdfdb");
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
