//! Tests of the external builder (`external_build`) that read its output back through the reader (`disk_snapshot`):
//! a module above both, so the builder itself depends on the reader for nothing (#107).
use super::external_build::*;
use super::disk_snapshot::adaptive;
use crate::{
    exec::{QueryResult, query},
    storage::DiskSnapshot,
};
/// Task #91: the deduplicating parse writes exactly the store the plain parse writes, with every term map budget from
/// "flush after every triple" to "never flush", including duplicate triples, blank nodes, repeated terms across
/// shards and shards that hold no triples.
#[test]
fn deduplicating_parse_writes_identical_stores() {
    crate::external_build::keep_orders_on_this_thread(crate::ALL_ORDERS);
    let root = std::env::temp_dir().join(format!("rdf-dedup-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root); fs::create_dir(&root).unwrap();
    let mut text = String::new();
    let mut x = 12345u64;
    let mut next = || { x ^= x << 13; x ^= x >> 7; x ^= x << 17; x };
    for i in 0..6000u64 {
        let s = next() % 400; let p = next() % 7; let o = next() % 900;
        match i % 5 {
            0 => text.push_str(&format!("<urn:s{s}> <urn:p{p}> <urn:o{o}> .\n")),
            1 => text.push_str(&format!("_:b{s} <urn:p{p}> \"v{o}\" .\n")),
            2 => text.push_str(&format!("<urn:s{s}> <urn:p{p}> \"{o}\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n")),
            3 => text.push_str(&format!("<urn:s{s}> <urn:p{p}> \"é{o}\"@fr .\n")),
            _ => text.push_str(&format!("<urn:s{}> <urn:p{}> <urn:o{}> .\n", s % 5, p % 2, o % 5)), // many duplicates
        }
    }
    let file = root.join("input.nt");
    fs::write(&file, &text).unwrap();
    let reference = root.join("plain");
    from_nt_parallel_with(&reference, Some(&file), 8, Dedup::Off).unwrap();
    for (label, mode) in [("one-byte", Dedup::Map(1)), ("small", Dedup::Map(4096)), ("large", Dedup::Map(256 << 20)), ("auto", Dedup::Auto)] {
        let out = root.join(label);
        from_nt_parallel_with(&out, Some(&file), 8, mode).unwrap();
        for name in FILES[1..].iter().copied().chain(["dictionary.unified", "MANIFEST"]) {
            assert_eq!(fs::read(reference.join(name)).unwrap(), fs::read(out.join(name)).unwrap(), "{label}: {name}");
        }
        assert!(!out.join(".rows-0").exists() && !out.join(".triples.raw").exists(), "{label}: scratch left behind");
        let snap = DiskSnapshot::open(&out).unwrap(); snap.verify().unwrap(); drop(snap);
    }
    // A file with fewer triples than shards: some shards are empty.
    let tiny = root.join("tiny.nt");
    fs::write(&tiny, "<urn:a> <urn:p> <urn:b> .\n<urn:a> <urn:p> <urn:b> .\n").unwrap();
    let (a, b) = (root.join("tiny-plain"), root.join("tiny-dedup"));
    from_nt_parallel_with(&a, Some(&tiny), 8, Dedup::Off).unwrap();
    from_nt_parallel_with(&b, Some(&tiny), 8, Dedup::Map(1)).unwrap();
    for name in FILES[1..].iter().copied().chain(["dictionary.unified", "MANIFEST"]) {
        assert_eq!(fs::read(a.join(name)).unwrap(), fs::read(b.join(name)).unwrap(), "tiny: {name}");
    }
    fs::remove_dir_all(root).unwrap();
}
/// RDF 1.2 through the deduplicating parse: nested triple terms, a part that occurs only inside one, and directional
/// literals give the same files with dedup off, with a one-byte map (an eviction after every triple) and by default.
#[cfg(feature = "rdf-12")]
#[test]
fn deduplicating_parse_keeps_rdf12_parts() {
    let root = std::env::temp_dir().join(format!("rdf-dedup12-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root); fs::create_dir(&root).unwrap();
    let mut text = String::new();
    for i in 0..400 {
        text += &format!("<urn:s{}> <urn:p> <<( <urn:a{}> <urn:inside> <<( _:b{} <urn:q> \"x{}\"@en--ltr )>> )>> .\n", i % 13, i % 7, i % 5, i % 3);
        text += &format!("<urn:s{}> <urn:label> \"v{}\"@ar--rtl .\n", i % 13, i % 11);
    }
    let file = root.join("input.nt");
    fs::write(&file, &text).unwrap();
    let reference = root.join("plain");
    from_nt_parallel_with(&reference, Some(&file), 8, Dedup::Off).unwrap();
    for (label, mode) in [("one-byte", Dedup::Map(1)), ("auto", Dedup::Auto)] {
        let out = root.join(label);
        from_nt_parallel_with(&out, Some(&file), 8, mode).unwrap();
        let mut names: Vec<_> = fs::read_dir(&reference).unwrap().map(|e| e.unwrap().file_name()).collect();
        names.sort();
        for name in names { assert_eq!(fs::read(reference.join(&name)).unwrap(), fs::read(out.join(&name)).unwrap(), "{label}: {name:?}") }
        DiskSnapshot::open(&out).unwrap().verify().unwrap();
    }
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn sharded_nt_preserves_terms_across_boundaries() {
    crate::external_build::keep_orders_on_this_thread(crate::ALL_ORDERS); // this test checks the six-order layout
    let root = std::env::temp_dir().join(format!("rdf-shards-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root); fs::create_dir(&root).unwrap();
    let file = root.join("input.nt");
    fs::write(&file, "# comment\r\n<urn:s> <urn:p> \"é\"@fr .\r\n\n_:a <urn:p> <urn:o> .\n<urn:s> <urn:p> \"é\"@fr .\n<urn:last> <urn:p> \"x\" .").unwrap();
    let a = root.join("serial"); let b = root.join("parallel");
    from_nt(&a, Some(&file)).unwrap(); from_nt_parallel(&b, Some(&file), 8).unwrap();
    for name in FILES[1..].iter().copied().chain(["dictionary.unified"]) { assert_eq!(fs::read(a.join(name)).unwrap(), fs::read(b.join(name)).unwrap(), "{name}"); }
    assert_eq!(fs::read(a.join("MANIFEST")).unwrap(), fs::read(b.join("MANIFEST")).unwrap());
    let snap = DiskSnapshot::open(&b).unwrap(); snap.verify().unwrap(); drop(snap);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn parallel_index_failure_does_not_publish_manifest() {
    let root = std::env::temp_dir().join(format!("rdf-index-failure-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root); fs::create_dir(&root).unwrap();
    let mut builder = Builder::new(&root).unwrap();
    builder.index_workers = 3;
    builder.add(["<urn:s>".into(), "<urn:p>".into(), "<urn:o>".into()]).unwrap();
    fs::write(root.join("spo"), b"collision").unwrap();
    assert!(builder.finish(&root).is_err());
    assert!(!root.join("MANIFEST").exists());
    assert!(!fs::read_dir(&root).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with(".rdf-sort-")));
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn sharded_blank_node_labels_are_document_scoped() {
    let root = std::env::temp_dir().join(format!("rdf-shards-bnode-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root); fs::create_dir(&root).unwrap();
    let input = root.join("input.nt");
    let mut data = String::from("_:x <urn:p> <urn:one> .\n");
    for i in 0..1000 { data.push_str(&format!("<urn:s{i}> <urn:p> <urn:o> .\n")); }
    data.push_str("_:x <urn:q> <urn:two> .\n");
    fs::write(&input, data).unwrap();
    let out = root.join("base"); from_nt_parallel(&out, Some(&input), 8).unwrap();
    let snap = DiskSnapshot::open(&out).unwrap();
    let result = query(&snap, "ASK { ?x <urn:p> <urn:one>; <urn:q> <urn:two> }").unwrap();
    assert!(matches!(result, QueryResult::Boolean(true)));
    drop(snap); fs::remove_dir_all(root).unwrap();
}
#[test]
fn sharded_nt_rejects_invalid_and_empty_works() {
    let root = std::env::temp_dir().join(format!("rdf-shards-invalid-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root); fs::create_dir(&root).unwrap();
    let file = root.join("input.nt");
    fs::write(&file, "<urn:a> <urn:p> <urn:o> .\nnot a triple\n").unwrap();
    let invalid = root.join("invalid");
    assert!(from_nt_parallel(&invalid, Some(&file), 8).is_err());
    assert!(!invalid.join("MANIFEST").exists());
    fs::write(&file, "").unwrap();
    let empty = root.join("empty"); from_nt_parallel(&empty, Some(&file), 8).unwrap();
    let snap = DiskSnapshot::open(&empty).unwrap(); snap.verify().unwrap(); drop(snap);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn ntriples_deduplicate_and_preserve_rdf_terms() {
    let dir = std::env::temp_dir().join(format!("rdf-ext-test-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let input = dir.join("input.nt");
    let output = dir.join("base");
    fs::write(&input,"<urn:s> <urn:p> \"hello\"@en .\n<urn:s> <urn:p> \"hello\"@en .\n_:x <urn:p> \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n").unwrap();
    from_nt(&output, Some(&input)).unwrap();
    let snapshot = DiskSnapshot::open(&output).unwrap();
    snapshot.verify().unwrap();
    let result = query(&snapshot, "SELECT ?s ?o WHERE { ?s <urn:p> ?o }").unwrap();
    match result {
        QueryResult::Solutions { rows, .. } => assert_eq!(rows.len(), 2),
        _ => panic!("unexpected result"),
    }
    drop(snapshot);
    fs::remove_dir_all(dir).unwrap();
}
fn small_input(dir: &Path) -> std::path::PathBuf {
    let input = dir.join("input.nt");
    let mut text = String::new();
    for i in 0..300 { text += &format!("<urn:s{i}> <urn:p{}> \"v{i}\" .\n<urn:s{i}> <urn:q> _:b{} .\n", i % 7, i % 11); }
    fs::write(&input, text).unwrap();
    input
}

/// Task #73: by default a new base keeps SPO, PSO, POS and OSP as `RDFSNAP6`, from the external builder and from
/// the adaptive one, which then skips its six-order in-memory path even for a small input.
#[test]
fn default_builds_keep_four_orders() {
    let dir = std::env::temp_dir().join(format!("rdf-ext-four-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    let input = small_input(&dir);
    for (label, out) in [("external", dir.join("external")), ("adaptive", dir.join("adaptive"))] {
        if label == "external" { from_nt(&out, Some(&input)).unwrap() } else { adaptive(&out, Some(&input), 1).unwrap() }
        let mut names: Vec<_> = fs::read_dir(&out).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        names.sort();
        assert_eq!(names, ["MANIFEST", "dictionary.unified", "osp", "pos", "pso", "spo"], "{label}");
        assert_eq!(&fs::read(out.join("MANIFEST")).unwrap()[..10], b"RDFSNAP6\x1d\x00", "{label}");
        DiskSnapshot::open(&out).unwrap().verify().unwrap();
    }
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn early_scratch_deletion_leaves_only_the_published_files() {
    crate::external_build::keep_orders_on_this_thread(crate::ALL_ORDERS); // this test checks the six-order layout
    let dir = std::env::temp_dir().join(format!("rdf-ext-clean-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let input = small_input(&dir);
    let out = dir.join("base");
    from_nt(&out, Some(&input)).unwrap();
    let mut names: Vec<_> = fs::read_dir(&out).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    assert_eq!(names, ["MANIFEST", "dictionary.unified", "ops", "osp", "pos", "pso", "sop", "spo"]);
    DiskSnapshot::open(&out).unwrap().verify().unwrap();
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failure_after_each_early_deletion_publishes_no_manifest() {
    for phase in 1..=3 {
        let dir = std::env::temp_dir().join(format!("rdf-ext-fail{phase}-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let input = small_input(&dir);
        let out = dir.join("base");
        FAIL_AT.with(|f| f.set(phase));
        let result = from_nt(&out, Some(&input));
        FAIL_AT.with(|f| f.set(0));
        assert!(result.is_err(), "phase {phase} should fail");
        assert!(!out.join("MANIFEST").exists(), "phase {phase} must not publish a manifest");
        assert!(DiskSnapshot::open(&out).is_err(), "phase {phase} must not open as a snapshot");
        fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn snapshot_format_fence_refuses_mismatched_manifests() {
    crate::external_build::keep_orders_on_this_thread(crate::ALL_ORDERS); // this test checks the six-order layout
    let dir = std::env::temp_dir().join(format!("rdf-ext-fence-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    let input = small_input(&dir);
    let out = dir.join("base");
    from_nt(&out, Some(&input)).unwrap();
    let manifest = fs::read(out.join("MANIFEST")).unwrap();
    assert_eq!(&manifest[..8], b"RDFSNAP4");
    assert_eq!(manifest.len(), 8 + 7 * 32, "one dictionary identity and six indexes");
    DiskSnapshot::open(&out).unwrap().verify().unwrap();
    // A reader that only knows RDFSNAP3 would expect eight identities and two dictionary files; the current code
    // path for 3 must refuse this store rather than misread it.
    let mut as_v3 = manifest.clone();
    as_v3[..8].copy_from_slice(b"RDFSNAP3");
    fs::write(out.join("MANIFEST"), &as_v3).unwrap();
    assert!(DiskSnapshot::open(&out).is_err(), "a v4 store read as v3 must be refused");
    // Unknown future versions are refused.
    let mut future = manifest.clone();
    future[..8].copy_from_slice(b"RDFSNAP9");
    fs::write(out.join("MANIFEST"), &future).unwrap();
    assert!(DiskSnapshot::open(&out).is_err());
    // Restoring the manifest makes it open again.
    fs::write(out.join("MANIFEST"), &manifest).unwrap();
    DiskSnapshot::open(&out).unwrap();
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn invalid_input_does_not_publish_a_manifest() {
    let parent = std::env::temp_dir().join(format!("rdf-ext-fail-{}", std::process::id()));
    fs::create_dir(&parent).unwrap();
    let input = parent.join("bad.nt");
    fs::write(&input, "<invalid\n").unwrap();
    let out = parent.join("base");
    assert!(from_nt(&out, Some(&input)).is_err());
    assert!(!out.join("MANIFEST").exists());
    fs::remove_dir_all(parent).unwrap();
}
