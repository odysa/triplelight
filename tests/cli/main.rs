//! Tests that start the `triplelight` binary or its server as a child process (one test target, so the binary builds once).
#[cfg(unix)]
mod disk_cli;
mod parallel_cli;
#[cfg(feature = "shacl")]
mod shacl_cli;
#[cfg(all(unix, feature = "server"))]
mod server_auth;

#[cfg(not(feature = "shacl"))]
#[test]
fn shacl_requires_the_feature() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_triplelight"))
        .args(["validate", "data.ttl", "shapes.ttl"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8(output.stderr).unwrap().contains("shacl Cargo feature"));
}
