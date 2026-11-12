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
