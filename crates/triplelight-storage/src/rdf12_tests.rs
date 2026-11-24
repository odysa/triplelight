//! RDF 1.2 storage tests (task #101, docs/rdf12-storage-design.md): triple terms and directional literals through the
//! in-memory store and its file format, the external disk build, WAL replay, compaction, and the `rdf12-1` fence.
use crate::model::Change;
use crate::storage::{Dictionary, MemorySnapshot, Snapshot};
use crate::store::Store;
use triplelight_rdf::{BaseDirection, BlankNode, Literal, NamedNode, Term, Triple};

fn iri(s: &str) -> NamedNode { NamedNode::new(format!("urn:{s}")).unwrap() }

/// A nested triple term, a triple term whose subject is a blank node also used at the top level, a part that occurs
/// only inside a triple term (`urn:only-inside`), a short inline-able string as a part, and directional and plain
/// literals with the same text and language.
fn sample() -> Vec<Triple> {
    let b = BlankNode::new("b1").unwrap();
    let inner = Triple::new(iri("a"), iri("only-inside"), Literal::new_simple_literal("x"));
    let nested = Triple::new(b.clone(), iri("says"), Term::Triple(Box::new(inner)));
    vec![
        Triple::new(iri("s"), iri("p"), Term::Triple(Box::new(nested))),
        Triple::new(b, iri("name"), Literal::new_simple_literal("bee")),
        Triple::new(iri("s"), iri("label"), Literal::new_language_tagged_literal("hi", "en").unwrap()),
        Triple::new(iri("s"), iri("label"), Literal::new_directional_language_tagged_literal("hi", "en", BaseDirection::Ltr).unwrap()),
        Triple::new(iri("s"), iri("label"), Literal::new_directional_language_tagged_literal("hi", "en", BaseDirection::Rtl).unwrap()),
    ]
}

/// Every sample triple is found by its IDs, every triple term's parts resolve to exactly its components (nested
/// ones too), and the three `hi@en` literals have three IDs.
fn check(view: &dyn Snapshot, label: &str) {
    assert!(view.has_rdf12_terms(), "{label}: 1.2 flag");
    for t in sample() {
        let ids: Vec<_> = [t.subject.clone().into(), t.predicate.clone().into(), t.object.clone()]
            .iter().map(|term: &Term| view.lookup(term).unwrap().unwrap_or_else(|| panic!("{label}: {term} missing"))).collect();
        assert_eq!(view.count(crate::model::Perm::Spo, &ids).unwrap(), 1, "{label}: {t}");
        let mut object = t.object.clone();
        while let Term::Triple(tt) = object {
            let id = view.lookup(&Term::Triple(tt.clone())).unwrap().unwrap();
            let parts = view.triple_term_parts(id).unwrap().unwrap_or_else(|| panic!("{label}: no parts for {tt}"));
            let expect: [Term; 3] = [tt.subject.clone().into(), tt.predicate.clone().into(), tt.object.clone()];
            for (part, term) in parts.iter().zip(&expect) { assert_eq!(&view.resolve(*part).unwrap(), term, "{label}: part of {tt}") }
            object = tt.object;
        }
        let pid = view.lookup(&t.predicate.clone().into()).unwrap().unwrap();
        assert_eq!(view.triple_term_parts(pid).unwrap(), None, "{label}: an IRI is not a triple term");
    }
    let labels: std::collections::HashSet<_> = sample()[2..].iter().map(|t| view.lookup(&t.object).unwrap().unwrap()).collect();
    assert_eq!(labels.len(), 3, "{label}: plain, ltr and rtl literals are distinct terms");
}

#[test]
fn memory_store_and_its_file_format() {
    let snapshot = MemorySnapshot::from_triples(sample());
    check(&snapshot, "memory");
    let mut bytes = Vec::new();
    snapshot.write_snapshot(&mut bytes).unwrap();
    check(&MemorySnapshot::read_snapshot(&bytes[..]).unwrap(), "memory reloaded");
    assert!(!MemorySnapshot::from_triples([Triple::new(iri("s"), iri("p"), iri("o"))]).has_rdf12_terms());
}

#[cfg(unix)]
#[test]
fn external_disk_build() {
    let dir = std::env::temp_dir().join(format!("rdf12-disk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).unwrap();
    let input = dir.join("input.nt");
    std::fs::write(&input, sample().iter().map(|t| format!("{t} .\n")).collect::<String>()).unwrap();
    let out = dir.join("base");
    crate::external_build::from_nt(&out, Some(&input)).unwrap();
    let disk = crate::DiskSnapshot::open(&out).unwrap();
    disk.verify().unwrap();
    assert_eq!(&std::fs::read(out.join("MANIFEST")).unwrap()[..10], b"RDFSNAP7\x1d\x00", "1.2 terms make an RDFSNAP7 base");
    assert!(out.join("triple_terms").exists(), "an RDFSNAP7 base has the parts table");
    check(&disk, "disk");
    let inside = disk.lookup(&iri("only-inside").into()).unwrap();
    assert!(inside.is_some(), "a part that occurs only inside a triple term is a dictionary term");
    assert_eq!(inside.unwrap().0 >> 60, 1);
    // A pre-#168 RDFSNAP7 manifest (no table identity) is refused with a message that says what to do.
    let manifest = std::fs::read(out.join("MANIFEST")).unwrap();
    let old = dir.join("old");
    std::fs::create_dir(&old).unwrap();
    for e in std::fs::read_dir(&out).unwrap() { let e = e.unwrap(); std::fs::copy(e.path(), old.join(e.file_name())).unwrap(); }
    std::fs::write(old.join("MANIFEST"), &manifest[..manifest.len() - 32]).unwrap();
    let err = crate::DiskSnapshot::open(&old).err().expect("an RDFSNAP7 base without the table is refused");
    assert!(err.to_string().contains("without a triple-term table"), "{err}");
    let tt = disk.lookup(&sample()[0].object).unwrap().unwrap();
    assert_eq!(tt.0 >> 60, 4, "triple terms have kind 4");
    std::fs::remove_dir_all(dir).unwrap();
}

/// Both base formats: N-Triples bases (`Store::open`) and disk bases (`DiskBase`). On the disk format, compaction
/// writes an RDFSNAP7 base, and the store still reports `has_rdf12_terms` from the base alone once the WAL is empty.
#[cfg(unix)]
#[test]
fn wal_replay_compaction_and_the_fence() {
    wal_case("nt", &|dir| Store::open(dir, None, 1).unwrap());
    wal_case("disk", &|dir| Store::open_with(dir, None, Box::new(crate::DiskBase { workers: 1 })).unwrap());
}
#[cfg(unix)]
fn wal_case(format: &str, open: &dyn Fn(&std::path::Path) -> Store) {
    let dir = std::env::temp_dir().join(format!("rdf12-store-{format}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let current = || std::fs::read_to_string(dir.join("CURRENT")).unwrap();
    {
        let store = open(&dir);
        store.commit(&[(Change::Insert, Triple::new(iri("s"), iri("p"), iri("o")))]).unwrap();
        assert!(!current().contains("rdf12-1 "), "a 1.1 record does not fence");
        assert!(!store.snapshot().has_rdf12_terms());
        let changes: Vec<_> = sample().into_iter().map(|t| (Change::Insert, t)).collect();
        store.commit(&changes).unwrap();
        assert!(current().contains("rdf12-1 "), "fenced before the first 1.2 record: {}", current());
        check(&*store.snapshot(), "delta");
    }
    {
        let store = open(&dir);
        check(&*store.snapshot(), "WAL replay");
        store.compact().unwrap();
        assert!(current().contains("rdf12-1 "), "compaction keeps the fence: {}", current());
        check(&*store.snapshot(), "compacted");
        assert_eq!(store.snapshot().delta_len(), 0);
        assert!(store.snapshot().base().has_rdf12_terms(), "{format}: the compacted base itself carries the 1.2 flag");
        if format == "disk" {
            let base = current().trim().rsplit(' ').next().unwrap().to_string();
            assert_eq!(&std::fs::read(dir.join(base).join("MANIFEST")).unwrap()[..8], b"RDFSNAP7");
        }
    }
    let store = open(&dir);
    check(&*store.snapshot(), "compacted and reopened");
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Commit refuses a triple term nested deeper than `MAX_TRIPLE_TERM_DEPTH` before applying anything, and accepts the
/// deepest allowed one, which survives WAL replay and compaction (whose dictionary pass parses it back).
#[cfg(unix)]
#[test]
fn commit_refuses_triple_terms_nested_too_deep() {
    let nest = |depth: usize| {
        let mut object: Term = iri("leaf").into();
        for _ in 0..depth { object = Term::Triple(Box::new(Triple::new(iri("a"), iri("b"), object))) }
        Triple::new(iri("s"), iri("deep"), object)
    };
    let max = triplelight_rdf::MAX_TRIPLE_TERM_DEPTH;
    let dir = std::env::temp_dir().join(format!("rdf12-depth-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    {
        let store = Store::open(&dir, None, 1).unwrap();
        let err = store.commit(&[(Change::Insert, iri_triple()), (Change::Insert, nest(max + 1))]).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(store.snapshot().count(crate::model::Perm::Spo, &[]).unwrap(), 0, "a refused commit applies nothing");
        store.commit(&[(Change::Insert, nest(max))]).unwrap();
    }
    let store = Store::open(&dir, None, 1).unwrap();
    store.compact().unwrap();
    let deepest = store.snapshot().lookup(&nest(max).object).unwrap();
    assert!(deepest.is_some(), "the deepest allowed term survives replay and compaction");
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

fn iri_triple() -> Triple { Triple::new(iri("s"), iri("p"), iri("o")) }
