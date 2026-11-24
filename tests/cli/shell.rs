use std::io::Write;
use std::process::{Command, Output, Stdio};

fn shell(args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_triplelight"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn stdout(out: Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn shell_preserves_sparql_terms_across_lines_and_statements() {
    let out = shell(
        &[],
        r#"
# semicolon in a comment ;
INSERT DATA {
  <urn:item;x> <urn:p> """first;
.mode json
last""" ;
    <urn:other> "value" .
}; # close the INSERT
SELECT ?s WHERE { ?s <urn:p> ?o }; ASK { <urn:item;x> <urn:other> "value" };
SELECT ?o WHERE { <urn:item;x> <urn:p> ?o }
"#,
    );
    let text = stdout(out);
    assert_eq!(
        text,
        concat!(
            "?s\n<urn:item;x>\n",
            "{\"head\":{},\"boolean\":true}\n",
            "?o\n\"first;\\n.mode json\\nlast\"\n",
        )
    );
}

#[test]
fn shell_reads_scripts_and_reopens_durable_stores() {
    let temp = std::env::temp_dir().join(format!("triplelight-shell-{}", std::process::id()));
    std::fs::create_dir(&temp).unwrap();
    let store = temp.join("my store");
    let data = temp.join("my data.nt");
    let script = temp.join("my script.rq");
    std::fs::write(&data, "<urn:item> <urn:p> \"original\" .\n").unwrap();
    std::fs::write(
        &script,
        format!(
            r#"
.open "{}"
.open "{}"
.import "{}"
INSERT DATA {{ <urn:item> <urn:p> "added" }};
.mode csv
SELECT ?o WHERE {{ <urn:item> <urn:p> ?o }} ORDER BY ?o;
"#,
            store.display(),
            store.display(),
            data.display()
        ),
    )
    .unwrap();
    let input = format!(
        ".read \"{}\"\n.quit\nSELECT * WHERE {{}};\n",
        script.display()
    );
    assert_eq!(
        stdout(shell(&["shell"], &input)),
        "o\r\nadded\r\noriginal\r\n"
    );

    let count = "SELECT (COUNT(*) AS ?n) WHERE { <urn:item> <urn:p> ?o }";
    let store = store.to_str().unwrap();
    assert_eq!(
        stdout(shell(&["shell", store, "--mode", "csv", "-c", count], "")),
        "n\r\n2\r\n"
    );

    let update = r#"INSERT DATA { <urn:item> <urn:p> "rolled back" }; DROP GRAPH <urn:absent>"#;
    let failed = shell(&["shell", store, "-c", update], "");
    assert!(!failed.status.success());
    assert_eq!(
        stdout(shell(&["shell", store, "--mode", "csv", "-c", count], "")),
        "n\r\n2\r\n"
    );
    std::fs::remove_dir_all(&temp).unwrap();
}

#[test]
fn shell_prints_box_and_keeps_timing_out_of_results() {
    let out = shell(
        &["shell"],
        ".mode box\n.timer on\nSELECT ?v WHERE { VALUES ?v { \"東\" } };\n",
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("Time: "));
    let text = stdout(out);
    assert!(text.contains("| ?v"), "{text}");
    assert!(text.contains("| \"東\" |"), "{text}");
    assert!(text.ends_with("(1 row)\n"), "{text}");
}

#[test]
fn shell_batch_stops_at_a_bad_statement() {
    let out = shell(&["shell"], "SELECT ???;\nASK {};\n");
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("line 1: syntax error"));
}
