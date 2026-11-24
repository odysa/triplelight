use std::process::Command;

/// `triplelight validate` (#108): the exit status keeps conforming data, a violation, a syntax error and a rejected
/// shapes graph apart, and the report is N-Triples on stdout.
#[test]
fn validate_exit_status_separates_conformance_violations_syntax_and_bad_shapes() {
    let root = std::env::temp_dir().join(format!("triplelight-shacl-cli-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let file = |name: &str, text: &str| { let p = root.join(name); std::fs::write(&p, text).unwrap(); p };
    let shapes = file("shapes.ttl", "@prefix sh: <http://www.w3.org/ns/shacl#> .\n<urn:S> a sh:NodeShape ; sh:targetClass <urn:Person> ;\n  sh:property [ sh:path <urn:name> ; sh:minCount 1 ] .\n");
    let good = file("good.nt", "<urn:ann> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Person> .\n<urn:ann> <urn:name> \"Ann\" .\n");
    let bad = file("bad.ttl", "<urn:bob> a <urn:Person> .\n");
    let broken = file("broken.ttl", "<urn:bob> a \n");
    let malformed = file("malformed.ttl", "@prefix sh: <http://www.w3.org/ns/shacl#> .\n<urn:S> a sh:NodeShape ; sh:targetClass <urn:Person> ;\n  sh:property [ sh:minCount 1 ] .\n");
    let run = |data: &std::path::Path, shapes: &std::path::Path| Command::new(env!("CARGO_BIN_EXE_triplelight")).arg("validate").arg(data).arg(shapes).output().unwrap();
    let ok = run(&good, &shapes);
    assert_eq!(ok.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&ok.stdout).contains("<http://www.w3.org/ns/shacl#conforms> \"true\"^^<http://www.w3.org/2001/XMLSchema#boolean>"));
    let violated = run(&bad, &shapes);
    assert_eq!(violated.status.code(), Some(1));
    let report = String::from_utf8_lossy(&violated.stdout);
    assert!(report.contains("<http://www.w3.org/ns/shacl#focusNode> <urn:bob>"), "{report}");
    assert!(report.contains("http://www.w3.org/ns/shacl#MinCountConstraintComponent"), "{report}");
    assert_eq!(run(&broken, &shapes).status.code(), Some(3));
    assert_eq!(run(&good, &broken).status.code(), Some(3));
    let rejected = run(&good, &malformed);
    assert_eq!(rejected.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("malformed shape"));
    assert_eq!(Command::new(env!("CARGO_BIN_EXE_triplelight")).arg("validate").arg(&good).output().unwrap().status.code(), Some(2));
    std::fs::remove_dir_all(root).unwrap();
}
