use std::process::Command;
#[test]
fn cli_default_explicit_serial_and_parallel_match_and_reject_invalid_options() {
    let root = std::env::temp_dir().join(format!("rdfdb-par-cli-{}",std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let nt=root.join("input.nt");let ttl=root.join("input.ttl");
    std::fs::write(&nt,"<urn:s> <urn:p> \"value\" .\n<urn:s2> <urn:p> \"value2\" .\n").unwrap();
    std::fs::write(&ttl,"<urn:s> <urn:p> \"value\" .\n").unwrap();
    let query="SELECT ?s ?o WHERE { ?s <urn:p> ?o }";
    let run=|path:&std::path::Path, opt:Option<&str>| {
        let mut command=Command::new(env!("CARGO_BIN_EXE_rdfdb"));command.arg("query").arg(path).arg(query);
        if let Some(opt)=opt {command.arg(opt);}command.output().unwrap()
    };
    let default=run(&nt,None);let serial=run(&nt,Some("--parallel-load=1"));let parallel=run(&nt,Some("--parallel-load=4"));
    assert!(default.status.success());assert!(serial.status.success());assert!(parallel.status.success());
    assert_eq!(default.stdout,serial.stdout);assert_eq!(serial.stdout,parallel.stdout);
    assert!(!run(&nt,Some("--parallel-load=0")).status.success());
    assert!(!run(&ttl,Some("--parallel-load=4")).status.success());
    assert!(run(&ttl,None).status.success());
    std::fs::remove_dir_all(root).unwrap();
}
