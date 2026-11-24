#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use triplelight::storage::{Dictionary, DiskBase, DiskSnapshot, Snapshot};
use triplelight::store::{Store, StoreUpdate};

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn text_index_preserves_store_bytes_and_sorts_unique_postings() {
    let dir = Fixture(std::env::temp_dir().join(format!("tl-text-index-{}", std::process::id())));
    fs::create_dir(&dir.0).unwrap();
    let nt = dir.0.join("seed.nt");
    fs::write(&nt, concat!(
        "<urn:a> <urn:label> \"Graph graph Straße\"@de .\n",
        "<urn:b> <urn:label> \"graph\" .\n",
        "<urn:c> <urn:label> \"42\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n",
    )).unwrap();
    let root = dir.0.join("database");
    let store = Store::open_with(&root, Some(&nt), Box::new(DiskBase { workers: 1 })).unwrap();
    let query = "PREFIX tl: <urn:triplelight:text:> SELECT ?s ?label WHERE { ?s <urn:label> ?label . FILTER(tl:matches(?label, \"STRASSE graph\")) } ORDER BY ?s";
    let answer = |store: &Store| {
        let snapshot = store.snapshot();
        let result = triplelight::exec::query(&*snapshot, query).unwrap();
        triplelight::exec::results::serialize(&result).1
    };
    let baseline = answer(&store);
    let current_before = fs::read(root.join("CURRENT")).unwrap();
    let manifest_before = fs::read(root.join("base-0/MANIFEST")).unwrap();
    let wal_before = fs::read(root.join("wal.log")).unwrap();
    assert_eq!(DiskSnapshot::build_text_index(&store, 2).unwrap(), 2);
    assert_eq!(fs::read(root.join("CURRENT")).unwrap(), current_before);
    assert_eq!(fs::read(root.join("base-0/MANIFEST")).unwrap(), manifest_before);
    assert_eq!(fs::read(root.join("wal.log")).unwrap(), wal_before);

    let file = fs::read(root.join("text-0.v2")).unwrap();
    let word = |offset: usize| u64::from_le_bytes(file[offset..offset + 8].try_into().unwrap()) as usize;
    assert_eq!(&file[..8], b"TLTEXT02");
    assert_eq!(word(8), 0);
    assert_eq!(word(16), 1);
    assert_eq!(word(72), 2);
    let directory = 112 + word(56);
    let postings2 = word(directory + 21);
    assert!(postings2 > 0 && postings2 < word(56));
    assert_eq!(word(directory + 8), 2);
    assert_eq!(&file[directory + 16..directory + 21], b"graph");
    assert_eq!(word(directory + 29), 1);
    assert_eq!(&file[directory + 37..directory + word(64)], b"strasse");
    assert_eq!(file.len(), directory + word(64) + 16);
    drop(store);

    let reopen = || Store::open_with(&root, None, Box::new(DiskBase { workers: 1 })).unwrap();
    let store = reopen();
    let pinned = store.snapshot();
    let a = pinned.lookup(&"\"Graph graph Straße\"@de".parse().unwrap()).unwrap().unwrap();
    let b = pinned.lookup(&"\"graph\"".parse().unwrap()).unwrap().unwrap();
    let both_words = pinned.text_candidates(&["graph", "strasse"], 8).unwrap();
    assert_eq!(both_words.matches(a), Some(true));
    assert_eq!(both_words.matches(b), Some(false));
    let graph_matches = |snapshot: &dyn Snapshot| {
        let candidates = snapshot.text_candidates(&["graph"], 8).unwrap();
        assert_eq!(candidates.matches(a), Some(true));
        assert_eq!(candidates.matches(b), Some(true));
    };
    graph_matches(&*pinned);
    assert!(pinned.text_candidates(&["graph"], 1).is_none());
    assert_eq!(answer(&store), baseline);

    drop(store);
    // A build with no oversized literals has the v1 payload layout byte-for-byte.
    let mut v1 = file.clone();
    v1[..8].copy_from_slice(b"TLTEXT01");
    fs::remove_file(root.join("text-0.v2")).unwrap();
    fs::write(root.join("text-0.v1"), v1).unwrap();
    let store = reopen();
    let legacy = store.snapshot();
    graph_matches(&*legacy);
    assert_eq!(answer(&store), baseline);
    assert_eq!(DiskSnapshot::build_text_index(&store, 2).unwrap(), 2);
    assert!(root.join("text-0.v2").is_file());
    assert!(!root.join("text-0.v1").exists());
    graph_matches(&*legacy);
    drop(store);

    let mut corrupt = file.clone();
    corrupt[112] ^= 1;
    let temp = root.join("index-replacement");
    let index = root.join("text-0.v2");
    fs::write(&temp, corrupt).unwrap();
    fs::rename(&temp, &index).unwrap();
    let store = reopen();
    assert!(store.snapshot().text_candidates(&["graph"], 8).is_none());
    assert_eq!(answer(&store), baseline);
    graph_matches(&*pinned);
    drop(store);

    fs::write(&temp, file).unwrap();
    fs::rename(temp, index).unwrap();
    let store = reopen();
    store.update("INSERT DATA { <urn:new> <urn:label> \"Graph Straße\" }").unwrap();
    assert!(store.snapshot().text_candidates(&["graph"], 8).is_none());
    let updated = answer(&store);
    assert!(updated.contains("urn:new"));
    store.compact().unwrap();
    assert!(!root.join("text-0.v2").exists());
    graph_matches(&*pinned);
    assert!(store.snapshot().text_candidates(&["graph"], 8).is_none());
    assert_eq!(answer(&store), updated);
    assert_eq!(DiskSnapshot::build_text_index(&store, 2).unwrap(), 2);
    drop(store);

    fs::write(root.join("text-0.v2"), "abandoned").unwrap();
    fs::write(root.join("text-0.v1"), "abandoned").unwrap();
    let store = reopen();
    assert!(!root.join("text-0.v2").exists());
    assert!(!root.join("text-0.v1").exists());
    let current = store.snapshot();
    let new = current.lookup(&r#""Graph Straße""#.parse().unwrap()).unwrap().unwrap();
    assert_eq!(current.text_candidates(&["graph"], 8).unwrap().matches(new), Some(true));
    graph_matches(&*pinned);
    assert_eq!(answer(&store), updated);
}

#[test]
fn oversized_literals_are_scalar_candidates_even_when_the_token_has_no_postings() {
    let dir = Fixture(std::env::temp_dir().join(format!("tl-text-oversized-{}", std::process::id())));
    fs::create_dir(&dir.0).unwrap();
    let nt = dir.0.join("seed.nt");
    let padding = "padding ".repeat(9_000);
    let hit = format!("{padding}needle");
    let miss = format!("{padding}haystack");
    fs::write(&nt, format!(
        "<urn:hit> <urn:label> \"{hit}\" .\n<urn:miss> <urn:label> \"{miss}\" .\n<urn:short> <urn:label> \"ordinary\" .\n",
    )).unwrap();
    let root = dir.0.join("database");
    let open = |initial| Store::open_with(&root, initial, Box::new(DiskBase { workers: 1 })).unwrap();
    let answer = |store: &Store, token: &str| {
        let query = format!(
            r#"PREFIX tl: <urn:triplelight:text:> SELECT ?s WHERE {{
                ?s <urn:label> ?label . FILTER(tl:matches(?label, "{token}"))
            }} ORDER BY ?s"#
        );
        let result = triplelight::exec::query(&*store.snapshot(), &query).unwrap();
        triplelight::exec::results::serialize(&result).1
    };

    let store = open(Some(&nt));
    let queries = ["needle", "absent", "ordinary", "padding"];
    let expected = queries.map(|token| answer(&store, token));
    assert!(expected[0].contains("urn:hit") && !expected[0].contains("urn:miss"));
    assert!(!expected[1].contains("urn:hit") && !expected[1].contains("urn:miss"));
    assert!(expected[2].contains("urn:short"));
    assert!(expected[3].contains("urn:hit") && expected[3].contains("urn:miss"));
    // One MiB caps each literal at 64 KiB, so both long literals must use the fallback list.
    assert_eq!(DiskSnapshot::build_text_index(&store, 1).unwrap(), 1);
    drop(store);

    let store = open(None);
    let snapshot = store.snapshot();
    let hit_id = snapshot.lookup(&triplelight::rdf::Literal::new_simple_literal(hit).into()).unwrap().unwrap();
    let short_id = snapshot.lookup(&r#""ordinary""#.parse().unwrap()).unwrap().unwrap();
    let no_posting = snapshot.text_candidates(&["needle"], 8).unwrap();
    assert_eq!(no_posting.matches(hit_id), None);
    assert_eq!(no_posting.matches(short_id), Some(false));
    assert!(snapshot.text_candidates(&["needle"], 1).is_none());
    assert_eq!(queries.map(|token| answer(&store, token)), expected);
}
