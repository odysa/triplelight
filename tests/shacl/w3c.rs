//! The W3C SHACL Core test suite (`data-shapes-test-suite/tests/core` of w3c/data-shapes), run through `triplelight::shacl::validate`.
//!
//! Pinned revision: w3c/data-shapes `eacdef4528d66ee116f0ac9118d968c5717c7233`. Point `TL_SHACL_TESTS` at the `tests/core` directory of an
//! exact checkout. Every `mf:include` is followed from `manifest.ttl`; each test file holds the test (`sht:Validate`), its data graph
//! and shapes graph (the file itself, or a file it names) and the expected report in `mf:result`.
//!
//! A result is compared by its focus node, value (when present), result path (as a parsed path, so blank-node path structure matches),
//! severity, source constraint component and source shape, as a multiset; `sh:conforms` must match. Messages are compared only when the
//! expected result has any (the suite asks implementations to preserve `sh:message` as `sh:resultMessage`). `sh:detail` is not compared.
//! Cases that are declared not to pass are listed with a reason in `w3c-exclusions.txt`: the run fails on an unexpected failure and
//! on an excluded case that now passes.
use std::collections::BTreeSet;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use triplelight::rdf::{NamedNode, Term, Triple};
use triplelight::shacl::{validate, Graph, Path as ShaclPath};
use triplelight::turtle::TurtleReader;

const MF: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#";
const SHT: &str = "http://www.w3.org/ns/shacl-test#";
const SH: &str = "http://www.w3.org/ns/shacl#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

fn iri(ns: &str, local: &str) -> NamedNode { NamedNode::new_unchecked(format!("{ns}{local}")) }

fn file_iri(p: &Path) -> String { format!("file://{}", p.display()) }

fn load(p: &Path) -> Vec<Triple> {
    let file = std::fs::File::open(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let reader = TurtleReader::with_base(BufReader::new(file), false, &file_iri(p)).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    reader.map(|q| q.unwrap_or_else(|e| panic!("{}: {e}", p.display()))).map(|q| Triple::new(q.subject, q.predicate, q.object)).collect()
}

fn path_of(iri: &str) -> PathBuf { PathBuf::from(iri.strip_prefix("file://").unwrap_or_else(|| panic!("not a file IRI: {iri}"))) }

/// Every test file reachable from the manifest, in a fixed order.
fn test_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.join("manifest.ttl")];
    let include = iri(MF, "include");
    while let Some(m) = stack.pop() {
        let g = Graph::new(&load(&m));
        let me = Term::from(NamedNode::new_unchecked(file_iri(&m)));
        let mut inc: Vec<PathBuf> = g.objects(&me, &include).iter().map(|t| match t { Term::NamedNode(n) => path_of(n.as_str()), _ => panic!("odd include") }).collect();
        inc.sort();
        for i in inc.into_iter().rev() {
            if i.file_name().is_some_and(|n| n == "manifest.ttl") { stack.push(i) } else { out.push(i) }
        }
    }
    out.sort();
    out
}

/// What a result is compared by.
fn key(g: &Graph, result: &Term, shapes_for_paths: &Graph) -> String {
    let one = |p: &str| g.objects(result, &iri(SH, p)).first().map(|t| t.to_string()).unwrap_or_default();
    let path = g.objects(result, &iri(SH, "resultPath")).first()
        .map(|p| format!("{:?}", ShaclPath::parse(p, shapes_for_paths).unwrap_or_else(|e| panic!("result path: {e}")))).unwrap_or_default();
    format!("focus={} | path={} | value={} | severity={} | component={} | shape={}", one("focusNode"), path, one("value"), one("resultSeverity"), one("sourceConstraintComponent"), one("sourceShape"))
}

fn messages(g: &Graph, result: &Term) -> BTreeSet<String> {
    g.objects(result, &iri(SH, "resultMessage")).iter().map(|t| t.to_string()).collect()
}

/// `Ok(())` when the report matches the expected one, else what differs.
fn run_case(file: &Path) -> Result<(), String> {
    let triples = load(file);
    let g = Graph::new(&triples);
    let test_type = Term::from(iri(SHT, "Validate"));
    let tests = g.subjects(&iri(RDF, "type"), &test_type);
    if tests.len() != 1 { return Err(format!("expected one sht:Validate test, found {}", tests.len())) }
    let test = &tests[0];
    let action = g.objects(test, &iri(MF, "action")).first().ok_or("no mf:action")?.clone();
    let named = |p: &str| match g.objects(&action, &iri(SHT, p)).first() {
        Some(Term::NamedNode(n)) => Ok(path_of(n.as_str())),
        _ => Err(format!("no sht:{p}")),
    };
    let (data_file, shapes_file) = (named("dataGraph")?, named("shapesGraph")?);
    let data = if data_file == file { triples.clone() } else { load(&data_file) };
    let shapes = if shapes_file == file { triples.clone() } else { load(&shapes_file) };
    let expected_root = g.objects(test, &iri(MF, "result")).first().ok_or("no mf:result")?.clone();
    let actual = validate(&data, &shapes).map_err(|e| format!("validation error: {e}"))?;
    let actual_triples = actual.to_triples();
    let ag = Graph::new(&actual_triples);
    let report_type = Term::from(iri(SH, "ValidationReport"));
    let report = ag.subjects(&iri(RDF, "type"), &report_type).into_iter().next().ok_or("no report node")?;

    let conforms = |gr: &Graph, r: &Term| gr.objects(r, &iri(SH, "conforms")).first().map(|t| t.to_string());
    if conforms(&g, &expected_root) != conforms(&ag, &report) {
        return Err(format!("sh:conforms: expected {:?}, got {:?}", conforms(&g, &expected_root), conforms(&ag, &report)));
    }
    let result_p = iri(SH, "result");
    let mut want: Vec<(String, BTreeSet<String>)> = g.objects(&expected_root, &result_p).iter().map(|r| (key(&g, r, &g), messages(&g, r))).collect();
    let mut got: Vec<(String, BTreeSet<String>)> = ag.objects(&report, &result_p).iter().map(|r| (key(&ag, r, &ag), messages(&ag, r))).collect();
    want.sort();
    got.sort();
    let keys = |v: &[(String, BTreeSet<String>)]| v.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
    if keys(&want) != keys(&got) {
        let (w, g2) = (keys(&want), keys(&got));
        let missing: Vec<_> = w.iter().filter(|k| !g2.contains(k)).collect();
        let extra: Vec<_> = g2.iter().filter(|k| !w.contains(k)).collect();
        return Err(format!("results differ ({} expected, {} reported)\n  missing: {missing:#?}\n  extra: {extra:#?}", w.len(), g2.len()));
    }
    for ((_, wm), (k, gm)) in want.iter().zip(&got) {
        if !wm.is_empty() && wm != gm { return Err(format!("messages differ for {k}: expected {wm:?}, got {gm:?}")) }
    }
    Ok(())
}

fn exclusions() -> BTreeSet<String> {
    let f = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shacl/w3c-exclusions.txt");
    std::fs::read_to_string(f).unwrap_or_default().lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| l.split_whitespace().next().unwrap().to_string()).collect()
}

#[test]
#[ignore = "requires the exact W3C data-shapes checkout (tests/core) in TL_SHACL_TESTS"]
fn w3c_shacl_core() {
    let root = PathBuf::from(std::env::var("TL_SHACL_TESTS").expect("TL_SHACL_TESTS must point at data-shapes-test-suite/tests/core"));
    let root = root.canonicalize().expect("the suite directory exists");
    let excluded = exclusions();
    let files = test_files(&root);
    assert!(files.len() > 50, "found only {} test files: wrong directory?", files.len());
    let (mut passed, mut unexpected, mut fixed) = (0, Vec::new(), Vec::new());
    for f in &files {
        let id = f.strip_prefix(&root).unwrap().with_extension("").display().to_string();
        match (run_case(f), excluded.contains(&id)) {
            (Ok(()), false) => passed += 1,
            (Ok(()), true) => fixed.push(id),
            (Err(_), true) => {}
            (Err(why), false) => unexpected.push(format!("{id}: {why}")),
        }
    }
    println!("SHACL Core: {} tests, {passed} passed, {} excluded, {} unexpected failures", files.len(), excluded.len(), unexpected.len());
    for u in &unexpected { println!("FAIL {u}") }
    for f in &fixed { println!("NOW PASSES (remove from the exclusion list): {f}") }
    assert!(unexpected.is_empty() && fixed.is_empty(), "{} unexpected failures, {} excluded cases now pass", unexpected.len(), fixed.len());
}
